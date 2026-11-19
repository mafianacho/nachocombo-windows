//! Nacho Pet login: Keycloak (realm `mafianacho`, the unified IdP), public client `nacho-gadget-app`, Authorization Code + PKCE (S256),
//! redirect `nachogadget://auth/callback` — the values nacho-server already accepts. Port of `PetAuth.swift`.
//! Everything that decides (URLs, PKCE, callback checks, token parsing, refresh timing) is here so it is tested on any
//! OS; the shell only opens the system browser ([`PetAuthBrowser`]), registers the `nachogadget:` URL scheme and keeps
//! the tokens in the credential store ([`PetSecretStore`]).
use super::crypto;
use super::http::{HTTPResult, PetHTTPClient, PetHTTPRequest};
use super::json::{self, JsonExt};
use super::secrets::{PetSecretLookup, PetSecretStore};
use super::settings::URLParts;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

/// OS random bytes (Windows: `BCryptGenRandom`); tests pass fixed bytes.
pub type RandomBytesSource = Arc<dyn Fn(usize) -> Vec<u8> + Send + Sync>;
/// Unix seconds now (the shell passes `clock::now`; tests pin it).
pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;
pub type TokenCompletion = Box<dyn FnOnce(Result<String, AuthProblem>) + Send>;
pub type AuthCompletion = Box<dyn FnOnce(Result<(), AuthProblem>) + Send>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OIDCConfig {
    /// Realm issuer, e.g. `https://auth.mafianacho.com/realms/mafianacho` (validated by `settings::IssuerURL`).
    pub issuer: String,
}

impl OIDCConfig {
    pub const CLIENT_ID: &'static str = "nacho-gadget-app";
    pub const REDIRECT_URI: &'static str = "nachogadget://auth/callback";
    pub const CALLBACK_SCHEME: &'static str = "nachogadget";
    pub const SCOPE: &'static str = "openid";

    pub fn new(issuer: String) -> Self { Self { issuer } }

    fn endpoint(&self, tail: &str) -> String { format!("{}/protocol/openid-connect/{tail}", self.issuer.trim_end_matches('/')) }
    pub fn authorization_endpoint(&self) -> String { self.endpoint("auth") }
    pub fn token_endpoint(&self) -> String { self.endpoint("token") }
    pub fn logout_endpoint(&self) -> String { self.endpoint("logout") }
}

pub struct Base64URL;

impl Base64URL {
    pub fn encode(bytes: &[u8]) -> String { crypto::base64url(bytes) }
}

/// RFC 7636. The verifier is 32 random bytes in base64url (43 characters, inside the 43...128 range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PKCE {
    pub verifier: String,
    pub challenge: String,
}

impl PKCE {
    pub const METHOD: &'static str = "S256";
    /// Random bytes a verifier needs (fewer would give a verifier under the 43-character minimum).
    pub const RANDOM_BYTES: usize = 32;

    pub fn new(verifier: impl Into<String>) -> Self {
        let verifier = verifier.into();
        let challenge = Self::challenge_for(&verifier);
        Self { verifier, challenge }
    }

    /// From OS random bytes: 32...96 of them (96 gives the 128-character maximum). None when fewer than 32 are given.
    pub fn from_random(bytes: &[u8]) -> Option<PKCE> {
        if bytes.len() < Self::RANDOM_BYTES { return None; }
        Some(Self::new(Base64URL::encode(&bytes[..bytes.len().min(96)])))
    }

    pub fn make(random: &RandomBytesSource) -> Option<PKCE> { Self::from_random(&random(Self::RANDOM_BYTES)) }

    pub fn challenge_for(verifier: &str) -> String { Base64URL::encode(&crypto::sha256(verifier.as_bytes())) }
}

/// One sign-in attempt: what was sent to the browser and must come back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSignIn {
    pub url: String,
    pub state: String,
    pub pkce: PKCE,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthProblem {
    NotConfigured,
    Canceled,
    Denied(String),
    StateMismatch,
    BadCallback,
    Network,
    Rejected(String),
    BadResponse,
    SignedOut,
    /// The credential store did not hand out the stored login. The login is kept; asked again later.
    SecretsUnavailable,
}

impl AuthProblem {
    pub fn korean_message(&self) -> &'static str {
        match self {
            Self::NotConfigured => "로그인 주소가 올바르지 않아요. 설정에서 확인해 주세요.",
            Self::Canceled => "로그인을 취소했어요.",
            Self::Denied(_) => "로그인이 거절됐어요. 다시 시도해 주세요.",
            Self::StateMismatch | Self::BadCallback => "로그인 응답이 올바르지 않아 무시했어요. 다시 시도해 주세요.",
            Self::Network => "로그인 서버에 연결하지 못했어요. 네트워크를 확인해 주세요.",
            Self::Rejected(_) => "로그인이 만료됐어요. 다시 로그인해 주세요.",
            Self::BadResponse => "로그인 서버 응답을 읽지 못했어요.",
            Self::SignedOut => "로그인이 필요해요.",
            // Windows wording: the macOS text speaks of the Keychain prompt and "항상 허용", which Windows does not have.
            Self::SecretsUnavailable => "Windows 자격 증명 관리자에서 저장된 로그인을 읽지 못했어요. 나초를 다시 열면 다시 읽어 볼게요.",
        }
    }
}

/// `%XX` → bytes → UTF-8 (`+` stays `+`, as in URL queries). None for a broken escape or bytes that are not UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `name=value` pairs of a query (value None when there is no `=`), percent-decoded.
pub fn query_items(query: &str) -> Option<Vec<(String, Option<String>)>> {
    query.split('&').filter(|p| !p.is_empty()).map(|p| match p.split_once('=') {
        Some((k, v)) => Some((percent_decode(k)?, Some(percent_decode(v)?))),
        None => Some((percent_decode(p)?, None)),
    }).collect()
}

pub struct OAuthRequests;

impl OAuthRequests {
    pub fn authorization_url(config: &OIDCConfig, pkce: &PKCE, state: &str) -> String {
        let items = [
            ("client_id", OIDCConfig::CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", OIDCConfig::REDIRECT_URI),
            ("scope", OIDCConfig::SCOPE),
            ("state", state),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", PKCE::METHOD),
        ];
        // Everything but RFC 3986 unreserved characters is escaped (also `:` and `/` of the redirect URI and `+`);
        // Keycloak compares the decoded value.
        let q: Vec<String> = items.iter().map(|(k, v)| format!("{k}={}", PetHTTPRequest::form_escape(v))).collect();
        format!("{}?{}", config.authorization_endpoint(), q.join("&"))
    }

    /// 32 random bytes for the verifier, 16 for `state`. None when the random source gives too few bytes.
    pub fn begin(config: &OIDCConfig, random: &RandomBytesSource) -> Option<PendingSignIn> {
        let pkce = PKCE::make(random)?;
        let state_bytes = random(16);
        if state_bytes.len() < 16 { return None; }
        let state = Base64URL::encode(&state_bytes[..16]);
        Some(PendingSignIn { url: Self::authorization_url(config, &pkce, &state), state, pkce })
    }

    /// Reads the browser's redirect. Only `nachogadget://auth/callback` with the expected `state` and a sane `code`
    /// passes; an `error` parameter (e.g. the user pressed cancel on the login page) is reported as such.
    pub fn code_from_callback(url: &str, expected_state: &str) -> Result<String, AuthProblem> {
        if url.chars().any(|c| c.is_whitespace() || c.is_control()) { return Err(AuthProblem::BadCallback); }
        let no_fragment = url.split('#').next().unwrap_or("");
        let (base, query) = match no_fragment.split_once('?') { Some((b, q)) => (b, q), None => (no_fragment, "") };
        let Some((parts, _, _)) = URLParts::split(base) else { return Err(AuthProblem::BadCallback) };
        if parts.scheme != OIDCConfig::CALLBACK_SCHEME || parts.host != "auth" || parts.path != "/callback" {
            return Err(AuthProblem::BadCallback);
        }
        let Some(items) = query_items(query) else { return Err(AuthProblem::BadCallback) };
        let value = |name: &str| -> Option<String> {
            let v: Vec<&Option<String>> = items.iter().filter(|(k, _)| k == name).map(|(_, v)| v).collect();
            if v.len() == 1 { v[0].clone() } else { None } // duplicated parameters are refused
        };
        if value("state").as_deref() != Some(expected_state) { return Err(AuthProblem::StateMismatch); }
        if let Some(e) = value("error") {
            return Err(if e == "access_denied" { AuthProblem::Canceled } else { AuthProblem::Denied(e) });
        }
        match value("code") {
            Some(c) if !c.is_empty() && c.chars().count() <= 4096 && c.chars().all(|ch| (ch as u32) > 0x20 && (ch as u32) < 0x7f) => Ok(c),
            _ => Err(AuthProblem::BadCallback),
        }
    }

    fn form(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    pub fn exchange(config: &OIDCConfig, code: &str, verifier: &str) -> PetHTTPRequest {
        PetHTTPRequest::post_form(config.token_endpoint(), &Self::form(&[
            ("grant_type", "authorization_code"), ("client_id", OIDCConfig::CLIENT_ID), ("code", code),
            ("redirect_uri", OIDCConfig::REDIRECT_URI), ("code_verifier", verifier),
        ]))
    }

    pub fn refresh(config: &OIDCConfig, refresh_token: &str) -> PetHTTPRequest {
        PetHTTPRequest::post_form(config.token_endpoint(), &Self::form(&[
            ("grant_type", "refresh_token"), ("client_id", OIDCConfig::CLIENT_ID), ("refresh_token", refresh_token),
        ]))
    }

    /// Keycloak ends the session (and invalidates the refresh token) on this call.
    pub fn logout(config: &OIDCConfig, refresh_token: &str) -> PetHTTPRequest {
        PetHTTPRequest::post_form(config.logout_endpoint(), &Self::form(&[("client_id", OIDCConfig::CLIENT_ID), ("refresh_token", refresh_token)]))
    }
}

/// Tokens of one signed-in account. Kept in the credential store as JSON (camelCase keys, times as Unix seconds —
/// the macOS app's Keychain item uses Apple reference-date seconds; the stores are never shared). Never logged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    /// Unix seconds.
    pub expires_at: f64,
    /// None = the refresh token has no announced end (Keycloak `refresh_expires_in: 0`).
    #[serde(default)]
    pub refresh_expires_at: Option<f64>,
}

impl TokenSet {
    /// Refreshed 60 s early so a WebSocket upgrade never starts with a token that dies on the way.
    pub const EARLY_REFRESH: f64 = 60.0;

    pub fn access_is_fresh(&self, now: f64) -> bool { now + Self::EARLY_REFRESH < self.expires_at }

    pub fn can_refresh(&self, now: f64) -> bool {
        if self.refresh_token.is_none() { return false; }
        self.refresh_expires_at.is_none_or(|end| now < end)
    }

    /// Token endpoint answer. Missing `expires_in` counts as 5 minutes; values are clamped to 10 s ... 1 day.
    pub fn decode(data: &[u8], now: f64) -> Result<TokenSet, AuthProblem> {
        let Some(o) = json::object(data) else { return Err(AuthProblem::BadResponse) };
        if let Some(e) = o.str("error") { return Err(AuthProblem::Rejected(e)); }
        let access = o.str("access_token").filter(|a| !a.is_empty() && a.chars().count() <= 16_384 && !a.contains(' ') && !a.contains('\n'));
        let Some(access) = access else { return Err(AuthProblem::BadResponse) };
        if o.str("token_type").is_some_and(|t| t.to_lowercase() != "bearer") { return Err(AuthProblem::BadResponse); }
        let seconds = |k: &str| o.get(k).and_then(serde_json::Value::as_f64);
        let expires_in = seconds("expires_in").unwrap_or(300.0).clamp(10.0, 86_400.0);
        let refresh = o.str("refresh_token").filter(|r| !r.is_empty() && r.chars().count() <= 16_384);
        let refresh_end = seconds("refresh_expires_in").filter(|r| *r > 0.0).map(|r| now + r.min(400.0 * 86_400.0));
        Ok(TokenSet { access_token: access, refresh_token: refresh, id_token: o.str("id_token"), expires_at: now + expires_in, refresh_expires_at: refresh_end })
    }

    /// What goes into the credential store.
    pub fn stored(&self) -> Vec<u8> { serde_json::to_vec(self).unwrap_or_default() }

    /// What comes back from it (None for anything unreadable).
    pub fn from_stored(data: &[u8]) -> Option<TokenSet> { serde_json::from_slice(data).ok() }
}

/// Opens the login page and returns the redirect URL (Windows: the default browser plus the registered
/// `nachogadget:` scheme handler; the dev CLI: plain HTTP against the mock server's stand-in login page).
pub trait PetAuthBrowser: Send + Sync {
    fn authorize(&self, url: &str, callback_scheme: &str, completion: Box<dyn FnOnce(Result<String, AuthProblem>) + Send>);
}

struct AuthState {
    tokens: Option<TokenSet>,
    /// Set while the secret store refused to hand out the stored login (not "signed out").
    store_problem: Option<i32>,
    refresh_waiters: Vec<TokenCompletion>,
}

struct Inner {
    config: Option<OIDCConfig>,
    scope: String,
    http: Arc<dyn PetHTTPClient>,
    secrets: Arc<dyn PetSecretStore>,
    random: RandomBytesSource,
    now: Clock,
    state: Mutex<AuthState>,
}

/// Token lifecycle for one account scope (server + realm). Thread-safe; completions run on whatever thread finished
/// the work (the caller's, or the HTTP client's), never while a lock is held.
#[derive(Clone)]
pub struct PetAuth {
    inner: Arc<Inner>,
}

impl PetAuth {
    /// Reads the stored login at once (a launch finds the session of the last one without any network call).
    pub fn new(config: Option<OIDCConfig>, scope: &str, http: Arc<dyn PetHTTPClient>, secrets: Arc<dyn PetSecretStore>,
               random: RandomBytesSource, now: Clock) -> Self {
        let inner = Inner {
            config, scope: scope.into(), http, secrets, random, now,
            state: Mutex::new(AuthState { tokens: None, store_problem: None, refresh_waiters: Vec::new() }),
        };
        inner.load_stored(&mut inner.state.lock().unwrap());
        Self { inner: Arc::new(inner) }
    }

    pub fn key(scope: &str) -> String { format!("tokens|{scope}") }

    pub fn config(&self) -> Option<&OIDCConfig> { self.inner.config.as_ref() }

    pub fn is_signed_in(&self) -> bool { self.inner.state.lock().unwrap().tokens.is_some() }

    /// No login in memory because the store refused to hand out the stored one (not "signed out").
    pub fn secrets_unavailable(&self) -> bool {
        let s = self.inner.state.lock().unwrap();
        s.tokens.is_none() && s.store_problem.is_some()
    }

    /// After a refused read: asks the secret store again. True when signed in.
    pub fn retry_stored_tokens(&self) -> bool {
        let mut s = self.inner.state.lock().unwrap();
        if s.tokens.is_none() && s.store_problem.is_some() { self.inner.load_stored(&mut s); }
        s.tokens.is_some()
    }

    /// Current access token without any network call (may be stale; the server answers 401 then).
    pub fn current_access_token(&self) -> Option<String> { self.inner.state.lock().unwrap().tokens.as_ref().map(|t| t.access_token.clone()) }

    pub fn begin(&self) -> Result<PendingSignIn, AuthProblem> {
        let c = self.inner.config.as_ref().ok_or(AuthProblem::NotConfigured)?;
        OAuthRequests::begin(c, &self.inner.random).ok_or(AuthProblem::NotConfigured)
    }

    /// Checks the redirect (scheme, path, `state`) and swaps the code for tokens with the PKCE verifier.
    pub fn complete(&self, callback: &str, pending: &PendingSignIn, completion: AuthCompletion) {
        let Some(c) = self.inner.config.clone() else { return completion(Err(AuthProblem::NotConfigured)) };
        match OAuthRequests::code_from_callback(callback, &pending.state) {
            Err(p) => completion(Err(p)),
            Ok(code) => {
                let weak = Arc::downgrade(&self.inner);
                self.inner.http.send(OAuthRequests::exchange(&c, &code, &pending.pkce.verifier), Box::new(move |result| {
                    let Some(inner) = weak.upgrade() else { return };
                    let r = inner.store(&mut inner.state.lock().unwrap(), result);
                    completion(r)
                }));
            }
        }
    }

    /// The whole sign-in: the stored login when the store hands it out now, otherwise the login page.
    pub fn sign_in(&self, browser: &dyn PetAuthBrowser, completion: AuthCompletion) {
        if self.retry_stored_tokens() { return completion(Ok(())); }
        let pending = match self.begin() { Ok(p) => p, Err(p) => return completion(Err(p)) };
        let auth = self.clone();
        let url = pending.url.clone();
        browser.authorize(&url, OIDCConfig::CALLBACK_SCHEME, Box::new(move |r| match r {
            Err(p) => completion(Err(p)),
            Ok(callback) => auth.complete(&callback, &pending, completion),
        }));
    }

    /// A usable access token: the stored one while fresh, otherwise one refresh (concurrent callers share it).
    pub fn valid_access_token(&self, completion: TokenCompletion) {
        let s = self.inner.state.lock().unwrap();
        let Some(t) = s.tokens.as_ref() else {
            let p = if s.store_problem.is_some() { AuthProblem::SecretsUnavailable } else { AuthProblem::SignedOut };
            drop(s);
            return completion(Err(p));
        };
        if t.access_is_fresh((self.inner.now)()) {
            let a = t.access_token.clone();
            drop(s);
            return completion(Ok(a));
        }
        Inner::refresh(&self.inner, s, completion)
    }

    /// Forces a refresh (the server said 401 although the token looked fresh).
    pub fn force_refresh(&self, completion: TokenCompletion) {
        let s = self.inner.state.lock().unwrap();
        if s.tokens.is_none() {
            let p = if s.store_problem.is_some() { AuthProblem::SecretsUnavailable } else { AuthProblem::SignedOut };
            drop(s);
            return completion(Err(p));
        }
        Inner::refresh(&self.inner, s, completion)
    }

    /// Ends the Keycloak session (best effort) and forgets the tokens here at once.
    pub fn sign_out(&self, completion: Box<dyn FnOnce() + Send>) {
        let rt = {
            let mut s = self.inner.state.lock().unwrap();
            let rt = s.tokens.as_ref().and_then(|t| t.refresh_token.clone());
            self.inner.clear(&mut s);
            rt
        };
        match (&self.inner.config, rt) {
            (Some(c), Some(r)) => self.inner.http.send(OAuthRequests::logout(c, &r), Box::new(move |_| completion())),
            _ => completion(),
        }
    }
}

impl Inner {
    /// A refused store read is remembered as `store_problem`, not as "signed out": the stored tokens stay where they
    /// are and are asked for again later.
    fn load_stored(&self, s: &mut AuthState) {
        match self.secrets.lookup(&PetAuth::key(&self.scope)) {
            PetSecretLookup::Found(d) => {
                s.tokens = TokenSet::from_stored(&d);
                s.store_problem = None;
            }
            PetSecretLookup::Missing => s.store_problem = None,
            PetSecretLookup::Unavailable(status) => s.store_problem = Some(status),
        }
    }

    fn clear(&self, s: &mut AuthState) {
        s.tokens = None;
        self.secrets.delete(&PetAuth::key(&self.scope));
    }

    fn refresh(this: &Arc<Inner>, mut s: std::sync::MutexGuard<'_, AuthState>, completion: TokenCompletion) {
        let now = (this.now)();
        let rt = s.tokens.as_ref().filter(|t| t.can_refresh(now)).and_then(|t| t.refresh_token.clone());
        let (Some(c), Some(rt)) = (this.config.clone(), rt) else {
            this.clear(&mut s);
            drop(s);
            return completion(Err(AuthProblem::Rejected("no_refresh".into())));
        };
        s.refresh_waiters.push(completion);
        if s.refresh_waiters.len() != 1 { return; } // one in flight: wait for its answer
        drop(s);
        let weak: Weak<Inner> = Arc::downgrade(this);
        this.http.send(OAuthRequests::refresh(&c, &rt), Box::new(move |result| {
            let Some(inner) = weak.upgrade() else { return };
            let (r, waiters) = {
                let mut s = inner.state.lock().unwrap();
                let r = match inner.store(&mut s, result) {
                    Ok(()) => Ok(s.tokens.as_ref().map(|t| t.access_token.clone()).unwrap_or_default()),
                    Err(p) => {
                        if matches!(p, AuthProblem::Rejected(_)) { inner.clear(&mut s); } // refresh token dead: signed out
                        Err(p)
                    }
                };
                (r, std::mem::take(&mut s.refresh_waiters))
            };
            for w in waiters { w(r.clone()); }
        }));
    }

    fn store(&self, s: &mut AuthState, result: HTTPResult) -> Result<(), AuthProblem> {
        let resp = result.map_err(|_| AuthProblem::Network)?;
        if resp.status >= 500 || resp.status == 0 { return Err(AuthProblem::Network); }
        let decoded = TokenSet::decode(&resp.body, (self.now)());
        if !resp.is_success() {
            if let Err(AuthProblem::Rejected(code)) = decoded { return Err(AuthProblem::Rejected(code)); }
            return Err(AuthProblem::Rejected(format!("http_{}", resp.status)));
        }
        let mut t = decoded?;
        if t.refresh_token.is_none() { t.refresh_token = s.tokens.as_ref().and_then(|o| o.refresh_token.clone()); } // Keycloak may omit it on refresh
        // A store that refuses keeps these in memory only (never written over what it holds); asked again next time.
        if self.secrets.write(&PetAuth::key(&self.scope), &t.stored()).is_ok() { s.store_problem = None; }
        s.tokens = Some(t);
        Ok(())
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod testing {
    //! Test doubles shared by the auth and connector tests.
    use super::*;
    use crate::core::pet::http::{HTTPCompletion, PetHTTPResponse, PetNetError};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

    /// Fixed bytes so verifiers and states are predictable: every call gives `count` copies of the next byte.
    pub fn fixed_random() -> RandomBytesSource {
        let next = Arc::new(AtomicU8::new(1));
        Arc::new(move |count| vec![next.fetch_add(1, Ordering::SeqCst); count])
    }

    /// Path of an absolute URL (without query).
    pub fn path_of(url: &str) -> String {
        let u = url.split(['?', '#']).next().unwrap_or("");
        URLParts::split(u).map(|(p, _, _)| p.path).unwrap_or_default()
    }

    /// Answers by "METHOD path". `on` sets the answer; `then` queues more (consumed in order, the last one repeats).
    /// `hold` keeps requests until `release` (to test concurrent callers).
    #[derive(Default)]
    pub struct PetStubHTTP {
        answers: Mutex<HashMap<String, Vec<PetHTTPResponse>>>,
        pub requests: Mutex<Vec<PetHTTPRequest>>,
        pub fail_network: AtomicBool,
        pub hold: AtomicBool,
        held: Mutex<Vec<(PetHTTPRequest, HTTPCompletion)>>,
    }

    impl PetStubHTTP {
        pub fn new() -> Arc<Self> { Arc::new(Self::default()) }
        pub fn on(&self, method: &str, path: &str, status: u16, body: &str) {
            self.answers.lock().unwrap().insert(format!("{method} {path}"), vec![PetHTTPResponse::new(status, body.as_bytes().to_vec())]);
        }
        pub fn then(&self, method: &str, path: &str, status: u16, body: &str) {
            self.answers.lock().unwrap().entry(format!("{method} {path}")).or_default().push(PetHTTPResponse::new(status, body.as_bytes().to_vec()));
        }
        pub fn requests(&self) -> Vec<PetHTTPRequest> { self.requests.lock().unwrap().clone() }
        pub fn last(&self) -> PetHTTPRequest { self.requests().last().cloned().expect("a request") }
        pub fn sent(&self, method: &str, path: &str) -> Vec<PetHTTPRequest> {
            self.requests().into_iter().filter(|r| r.method == method && path_of(&r.url) == path).collect()
        }
        fn answer(&self, r: &PetHTTPRequest) -> HTTPResult {
            if self.fail_network.load(Ordering::SeqCst) { return Err(PetNetError("offline".into())); }
            let key = format!("{} {}", r.method, path_of(&r.url));
            let mut a = self.answers.lock().unwrap();
            let list = a.entry(key).or_default();
            let resp = list.first().cloned().unwrap_or_else(|| PetHTTPResponse::new(404, b"{}".to_vec()));
            if list.len() > 1 { list.remove(0); }
            Ok(resp)
        }
        pub fn release(&self) {
            let held: Vec<_> = std::mem::take(&mut *self.held.lock().unwrap());
            for (r, c) in held { let a = self.answer(&r); c(a); }
        }
        pub fn form(r: &PetHTTPRequest) -> HashMap<String, String> {
            let body = String::from_utf8(r.body.clone().unwrap_or_default()).unwrap();
            query_items(&body).unwrap_or_default().into_iter().map(|(k, v)| (k, v.unwrap_or_default())).collect()
        }
        pub fn json(r: &PetHTTPRequest) -> json::JsonObject { r.body_json().unwrap_or_default() }
    }

    impl PetHTTPClient for PetStubHTTP {
        fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion) {
            self.requests.lock().unwrap().push(request.clone());
            if self.hold.load(Ordering::SeqCst) { return self.held.lock().unwrap().push((request, completion)); }
            let a = self.answer(&request);
            completion(a);
        }
    }

    /// Answers the login page with a redirect carrying the state it was given.
    #[derive(Default)]
    pub struct PetFakeBrowser {
        pub opened: Mutex<Vec<String>>,
        pub cancel: AtomicBool,
    }

    impl PetFakeBrowser {
        pub fn opened(&self) -> usize { self.opened.lock().unwrap().len() }
    }

    impl PetAuthBrowser for PetFakeBrowser {
        fn authorize(&self, url: &str, _callback_scheme: &str, completion: Box<dyn FnOnce(Result<String, AuthProblem>) + Send>) {
            self.opened.lock().unwrap().push(url.into());
            if self.cancel.load(Ordering::SeqCst) { return completion(Err(AuthProblem::Canceled)); }
            let q = url.split_once('?').map(|(_, q)| q).unwrap_or("");
            let state = query_items(q).unwrap().into_iter().find(|(k, _)| k == "state").and_then(|(_, v)| v).unwrap();
            completion(Ok(format!("nachogadget://auth/callback?code=c1&state={state}")))
        }
    }

    pub const TOKEN_JSON: &str = r#"{"access_token":"at-1","token_type":"Bearer","expires_in":300,"refresh_token":"rt-1","refresh_expires_in":1800,"id_token":"id-1"}"#;
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::core::pet::secrets::testing::ScriptedStore;
    use crate::core::pet::secrets::{MemorySecretStore, PetSecretBundle};
    use std::sync::mpsc;

    const TOKEN_PATH: &str = "/realms/mafianacho/protocol/openid-connect/token";

    fn config() -> OIDCConfig { OIDCConfig::new("https://auth.mafianacho.com/realms/mafianacho".into()) }

    /// A clock the test moves.
    fn clock(start: f64) -> (Arc<Mutex<f64>>, Clock) {
        let now = Arc::new(Mutex::new(start));
        let n = now.clone();
        (now, Arc::new(move || *n.lock().unwrap()))
    }

    fn advance(now: &Arc<Mutex<f64>>, by: f64) { *now.lock().unwrap() += by; }

    fn auth(http: &Arc<PetStubHTTP>, secrets: Arc<dyn PetSecretStore>, scope: &str, now: Clock) -> PetAuth {
        PetAuth::new(Some(config()), scope, http.clone(), secrets, fixed_random(), now)
    }

    fn signed_in(http: &Arc<PetStubHTTP>, secrets: Arc<dyn PetSecretStore>, now: Clock) -> PetAuth {
        http.on("POST", TOKEN_PATH, 200, TOKEN_JSON);
        let a = auth(http, secrets, "s", now);
        let pending = a.begin().unwrap();
        let (tx, rx) = mpsc::channel();
        a.complete(&format!("nachogadget://auth/callback?code=c1&state={}", pending.state), &pending, Box::new(move |r| tx.send(r).unwrap()));
        assert_eq!(rx.recv().unwrap(), Ok(()), "sign-in failed");
        let form = PetStubHTTP::form(&http.last());
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "c1");
        assert_eq!(form["code_verifier"], pending.pkce.verifier);
        assert_eq!(form["redirect_uri"], "nachogadget://auth/callback");
        assert_eq!(form["client_id"], "nacho-gadget-app");
        a
    }

    fn token(a: &PetAuth) -> Result<String, AuthProblem> {
        let (tx, rx) = mpsc::channel();
        a.valid_access_token(Box::new(move |r| tx.send(r).unwrap()));
        rx.recv().unwrap()
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        let p = PKCE::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let made = PKCE::make(&fixed_random()).unwrap();
        assert_eq!(made.verifier.len(), 43);
        assert!(!made.verifier.contains('='));
        assert!(!made.verifier.contains('+') && !made.verifier.contains('/'));
        assert_eq!(PKCE::from_random(&[7; 31]), None, "too few random bytes");
        assert_eq!(PKCE::from_random(&[7; 200]).unwrap().verifier.len(), 128, "capped at the RFC maximum");
    }

    #[test]
    fn authorization_url_carries_the_server_accepted_values() {
        let pending = OAuthRequests::begin(&config(), &fixed_random()).unwrap();
        let (base, query) = pending.url.split_once('?').unwrap();
        assert_eq!(base, "https://auth.mafianacho.com/realms/mafianacho/protocol/openid-connect/auth");
        let q: BTreeMap<String, String> = query_items(query).unwrap().into_iter().map(|(k, v)| (k, v.unwrap())).collect();
        assert_eq!(q["client_id"], "nacho-gadget-app");
        assert_eq!(q["redirect_uri"], "nachogadget://auth/callback");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], pending.pkce.challenge);
        assert_eq!(q["state"], pending.state);
        assert_eq!(q["scope"], "openid");
        assert_ne!(pending.state, pending.pkce.verifier);
        assert_eq!(OIDCConfig::new("https://a.example/realms/x/".into()).token_endpoint(), "https://a.example/realms/x/protocol/openid-connect/token");
    }

    #[test]
    fn callback_checks() {
        let cb = |s: &str| OAuthRequests::code_from_callback(s, "st");
        use AuthProblem::*;
        assert_eq!(cb("nachogadget://auth/callback?code=abc&state=st&session_state=x"), Ok("abc".into()));
        assert_eq!(cb("nachogadget://auth/callback?code=abc&state=other"), Err(StateMismatch));
        assert_eq!(cb("nachogadget://auth/callback?code=abc"), Err(StateMismatch));
        assert_eq!(cb("nachogadget://auth/callback?error=access_denied&state=st"), Err(Canceled));
        assert_eq!(cb("nachogadget://auth/callback?error=login_required&state=st"), Err(Denied("login_required".into())));
        assert_eq!(cb("nachogadget://auth/callback?error=access_denied&state=evil"), Err(StateMismatch));
        assert_eq!(cb("nachogadget://evil/callback?code=abc&state=st"), Err(BadCallback));
        assert_eq!(cb("https://auth/callback?code=abc&state=st"), Err(BadCallback));
        assert_eq!(cb("nachogadget://auth/callback?code=a&code=b&state=st"), Err(BadCallback));
        assert_eq!(cb("nachogadget://auth/callback?code=&state=st"), Err(BadCallback));
        assert_eq!(cb("nachogadget://auth/callback?code=a%20b&state=st"), Err(BadCallback));
        assert_eq!(cb("NachoGadget://AUTH/callback?code=abc&state=st#frag"), Ok("abc".into()));
        assert_eq!(cb("nachogadget://auth/callback?code=a%zz&state=st"), Err(BadCallback));
    }

    #[test]
    fn token_decoding() {
        let now = 1_000.0;
        let t = TokenSet::decode(TOKEN_JSON.as_bytes(), now).unwrap();
        assert_eq!(t.access_token, "at-1");
        assert_eq!(t.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(t.expires_at, now + 300.0);
        assert_eq!(t.refresh_expires_at, Some(now + 1800.0));
        assert!(t.access_is_fresh(now + 200.0));
        assert!(!t.access_is_fresh(now + 250.0), "refreshed a minute early");
        assert!(t.can_refresh(now + 1799.0));
        assert!(!t.can_refresh(now + 1800.0));
        assert_eq!(TokenSet::decode(br#"{"error":"invalid_grant"}"#, now), Err(AuthProblem::Rejected("invalid_grant".into())));
        assert_eq!(TokenSet::decode(br#"{"access_token":"a","token_type":"mac"}"#, now), Err(AuthProblem::BadResponse));
        assert_eq!(TokenSet::decode(br#"{"access_token":"a b"}"#, now), Err(AuthProblem::BadResponse));
        assert_eq!(TokenSet::decode(b"nope", now), Err(AuthProblem::BadResponse));
        let offline = TokenSet::decode(br#"{"access_token":"a","expires_in":1,"refresh_token":"r","refresh_expires_in":0}"#, now).unwrap();
        assert_eq!(offline.expires_at, now + 10.0, "clamped");
        assert_eq!(offline.refresh_expires_at, None);
        assert!(offline.can_refresh(now + 1_000_000.0));
        assert_eq!(TokenSet::from_stored(&t.stored()), Some(t));
    }

    #[test]
    fn sign_in_stores_tokens_and_refreshes_when_stale() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let secrets = Arc::new(MemorySecretStore::new());
        let a = signed_in(&http, secrets.clone(), clk.clone());
        assert!(secrets.read("tokens|s").is_some(), "kept in the secret store");
        assert_eq!(auth(&http, secrets.clone(), "s", clk.clone()).current_access_token().as_deref(), Some("at-1"), "a new launch finds the session");
        assert_eq!(auth(&http, secrets.clone(), "other", clk.clone()).current_access_token(), None, "another server never sees these tokens");
        assert_eq!(token(&a), Ok("at-1".into()));
        assert_eq!(http.requests().len(), 1, "fresh: no network");
        advance(&now, 290.0);
        http.on("POST", TOKEN_PATH, 200, r#"{"access_token":"at-2","expires_in":300}"#);
        assert_eq!(token(&a), Ok("at-2".into()));
        let form = PetStubHTTP::form(&http.last());
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["refresh_token"], "rt-1");
        advance(&now, 290.0);
        assert_eq!(token(&a), Ok("at-2".into()), "repeated last answer; the old refresh token was kept");
        assert_eq!(PetStubHTTP::form(&http.last())["refresh_token"], "rt-1");
    }

    #[test]
    fn concurrent_refresh_shares_one_request() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let a = signed_in(&http, Arc::new(MemorySecretStore::new()), clk);
        advance(&now, 1000.0);
        let before = http.requests().len();
        http.hold.store(true, std::sync::atomic::Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let threads: Vec<_> = (0..5).map(|_| {
            let (a, tx) = (a.clone(), tx.clone());
            std::thread::spawn(move || a.valid_access_token(Box::new(move |r| tx.send(r).unwrap())))
        }).collect();
        for t in threads { t.join().unwrap(); }
        assert_eq!(http.requests().len() - before, 1);
        http.release();
        for _ in 0..5 { assert_eq!(rx.recv().unwrap(), Ok("at-1".into())); }
    }

    #[test]
    fn dead_refresh_token_signs_out() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let secrets = Arc::new(MemorySecretStore::new());
        let a = signed_in(&http, secrets.clone(), clk);
        advance(&now, 1000.0);
        http.on("POST", TOKEN_PATH, 400, r#"{"error":"invalid_grant"}"#);
        assert_eq!(token(&a), Err(AuthProblem::Rejected("invalid_grant".into())));
        assert!(!a.is_signed_in());
        assert!(secrets.read("tokens|s").is_none());
        assert_eq!(token(&a), Err(AuthProblem::SignedOut));
    }

    #[test]
    fn network_failure_keeps_the_session() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let a = signed_in(&http, Arc::new(MemorySecretStore::new()), clk);
        advance(&now, 1000.0);
        http.fail_network.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(token(&a), Err(AuthProblem::Network));
        assert!(a.is_signed_in(), "offline is not signed out");
    }

    #[test]
    fn sign_out_ends_the_keycloak_session_and_forgets() {
        let http = PetStubHTTP::new();
        let secrets = Arc::new(MemorySecretStore::new());
        let a = signed_in(&http, secrets.clone(), Arc::new(crate::core::pet::clock::now));
        let (tx, rx) = mpsc::channel();
        a.sign_out(Box::new(move || tx.send(()).unwrap()));
        rx.recv().unwrap();
        let logout = http.sent("POST", "/realms/mafianacho/protocol/openid-connect/logout");
        assert_eq!(logout.len(), 1);
        assert_eq!(PetStubHTTP::form(&logout[0])["refresh_token"], "rt-1");
        assert!(!a.is_signed_in());
        assert!(secrets.read("tokens|s").is_none());
    }

    #[test]
    fn form_encoding_escapes_reserved_characters() {
        assert_eq!(PetHTTPRequest::form_escape("a+b&c=d e/é"), "a%2Bb%26c%3Dd%20e%2F%C3%A9");
    }

    /// Regression (macOS bug: users had to log in again after every restart). An app restart is a new PetAuth over
    /// the same store: it must be signed in without the login page, use the stored access token while fresh, refresh
    /// once when stale, and never delete the stored tokens on start.
    #[test]
    fn restart_keeps_the_session_without_a_new_login() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let kc = ScriptedStore::new();
        let store: Arc<dyn PetSecretStore> = Arc::new(PetSecretBundle::new(kc.clone())); // what the app uses
        let browser = PetFakeBrowser::default();
        http.on("POST", TOKEN_PATH, 200, TOKEN_JSON);
        let first = auth(&http, store, "s", clk.clone());
        let (tx, rx) = mpsc::channel();
        first.sign_in(&browser, Box::new(move |r| tx.send(r).unwrap()));
        assert_eq!(rx.recv().unwrap(), Ok(()));
        assert_eq!(browser.opened(), 1, "the first launch shows the login page");
        drop(first);

        // Restart: a new store object over the same credential store and a new PetAuth.
        let store: Arc<dyn PetSecretStore> = Arc::new(PetSecretBundle::new(kc.clone()));
        let a = auth(&http, store, "s", clk.clone());
        assert!(a.is_signed_in(), "restored at launch");
        assert!(!a.secrets_unavailable());
        assert!(kc.deleted().is_empty(), "nothing deleted on start");
        let (tx, rx) = mpsc::channel();
        a.sign_in(&browser, Box::new(move |r| tx.send(r).unwrap()));
        assert_eq!(rx.recv().unwrap(), Ok(()));
        assert_eq!(browser.opened(), 1, "no login page after a restart");

        let before = http.sent("POST", TOKEN_PATH).len();
        assert_eq!(token(&a), Ok("at-1".into()), "the stored access token while fresh");
        assert_eq!(http.sent("POST", TOKEN_PATH).len(), before, "fresh: no network");

        advance(&now, 600.0); // access (300 s) expired, refresh (1800 s) alive
        http.on("POST", TOKEN_PATH, 200, r#"{"access_token":"at-2","expires_in":300,"refresh_token":"rt-2","refresh_expires_in":1800}"#);
        assert_eq!(token(&a), Ok("at-2".into()));
        let refreshes = http.sent("POST", TOKEN_PATH);
        assert_eq!(refreshes.len(), before + 1, "one token request");
        assert_eq!(PetStubHTTP::form(refreshes.last().unwrap())["grant_type"], "refresh_token");
        assert!(kc.deleted().is_empty());
        assert_eq!(browser.opened(), 1);
    }

    #[test]
    fn restart_with_an_expired_access_token_refreshes_silently() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let kc = ScriptedStore::new();
        signed_in(&http, Arc::new(PetSecretBundle::new(kc.clone())), clk.clone());
        advance(&now, 600.0);
        http.on("POST", TOKEN_PATH, 200, r#"{"access_token":"at-2","expires_in":300,"refresh_token":"rt-2","refresh_expires_in":1800}"#);
        let a = auth(&http, Arc::new(PetSecretBundle::new(kc.clone())), "s", clk.clone());
        assert_eq!(token(&a), Ok("at-2".into()));
        assert_eq!(PetStubHTTP::form(&http.last())["refresh_token"], "rt-1");
        // And again on the launch after that, with the rotated refresh token.
        advance(&now, 600.0);
        http.on("POST", TOKEN_PATH, 200, r#"{"access_token":"at-3","expires_in":300,"refresh_token":"rt-3","refresh_expires_in":1800}"#);
        let again = auth(&http, Arc::new(PetSecretBundle::new(kc.clone())), "s", clk);
        assert_eq!(token(&again), Ok("at-3".into()));
        assert_eq!(PetStubHTTP::form(&http.last())["refresh_token"], "rt-2");
    }

    #[test]
    fn restart_when_the_refresh_is_rejected_asks_to_sign_in() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let secrets = Arc::new(MemorySecretStore::new());
        signed_in(&http, secrets.clone(), clk.clone());
        advance(&now, 600.0);
        http.on("POST", TOKEN_PATH, 400, r#"{"error":"invalid_grant","error_description":"Session not active"}"#);
        let a = auth(&http, secrets.clone(), "s", clk);
        assert_eq!(token(&a), Err(AuthProblem::Rejected("invalid_grant".into())));
        assert!(!a.is_signed_in());
        assert!(secrets.read("tokens|s").is_none(), "the dead login is forgotten");
    }

    #[test]
    fn restart_after_the_refresh_lifetime_asks_to_sign_in_without_calling_the_server() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let secrets = Arc::new(MemorySecretStore::new());
        signed_in(&http, secrets.clone(), clk.clone());
        let before = http.sent("POST", TOKEN_PATH).len();
        advance(&now, 1900.0); // refresh_expires_in 1800 passed while the app was closed
        let a = auth(&http, secrets, "s", clk);
        assert_eq!(token(&a), Err(AuthProblem::Rejected("no_refresh".into())));
        assert!(!a.is_signed_in());
        assert_eq!(http.sent("POST", TOKEN_PATH).len(), before);
    }

    #[test]
    fn restart_offline_keeps_the_login() {
        let (now, clk) = clock(1_000.0);
        let http = PetStubHTTP::new();
        let secrets = Arc::new(MemorySecretStore::new());
        signed_in(&http, secrets.clone(), clk.clone());
        advance(&now, 600.0);
        http.fail_network.store(true, std::sync::atomic::Ordering::SeqCst);
        let a = auth(&http, secrets.clone(), "s", clk);
        assert_eq!(token(&a), Err(AuthProblem::Network));
        assert!(a.is_signed_in());
        assert!(secrets.read("tokens|s").is_some());
    }

    #[test]
    fn pet_auth_tells_a_refused_keychain_from_no_login() {
        let http = PetStubHTTP::new();
        let kc = ScriptedStore::new();
        let blob = br#"{"accessToken":"at-old","refreshToken":"rt-old","expiresAt":0,"refreshExpiresAt":null}"#;
        kc.put("tokens|s", blob);
        kc.refuse(Some(-128));
        let a = auth(&http, kc.clone(), "s", Arc::new(|| 1_000.0));
        assert!(!a.is_signed_in());
        assert!(a.secrets_unavailable());
        assert_eq!(token(&a), Err(AuthProblem::SecretsUnavailable));
        assert!(http.requests().is_empty(), "no refresh, no login");
        assert_eq!(kc.get("tokens|s").as_deref(), Some(&blob[..]));
        assert!(kc.deleted().is_empty());
        assert!(!a.retry_stored_tokens(), "still refused");
        kc.refuse(None);
        assert!(a.retry_stored_tokens());
        assert!(!a.secrets_unavailable());
        assert_eq!(a.current_access_token().as_deref(), Some("at-old"));
        assert!(!auth(&http, kc.clone(), "none", Arc::new(|| 1_000.0)).secrets_unavailable(), "missing is not refused");
    }

    #[test]
    fn sign_in_via_the_browser_and_cancel() {
        let http = PetStubHTTP::new();
        http.on("POST", TOKEN_PATH, 200, TOKEN_JSON);
        let browser = PetFakeBrowser::default();
        browser.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        let a = auth(&http, Arc::new(MemorySecretStore::new()), "s", Arc::new(|| 1_000.0));
        let (tx, rx) = mpsc::channel();
        a.sign_in(&browser, Box::new(move |r| tx.send(r).unwrap()));
        assert_eq!(rx.recv().unwrap(), Err(AuthProblem::Canceled));
        assert!(!a.is_signed_in());
        let none = PetAuth::new(None, "s", http.clone(), Arc::new(MemorySecretStore::new()), fixed_random(), Arc::new(|| 0.0));
        assert_eq!(none.begin(), Err(AuthProblem::NotConfigured));
    }
}
