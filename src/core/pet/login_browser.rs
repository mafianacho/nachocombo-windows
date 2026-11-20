//! The login page and its way back on Windows: [`PetAuthBrowser`] for the system browser and the `nachogadget:` link.
//!
//! The link scheme is registered in the current user's registry only (no administrator rights), and only when the user
//! presses the login button ([`PetCallbackBrowser::authorize`] calls `prepare`). The browser then opens the redirect
//! address, Windows starts a second copy of the exe with that address as its argument, the copy hands the address to
//! the running one and quits; [`PetCallbackBrowser::deliver`] gives it to the waiting sign-in. The `state` and the PKCE
//! check happen in `PetAuth::complete`, so an address nobody asked for does nothing.
use super::auth::{AuthProblem, OIDCConfig, PetAuthBrowser};
use std::sync::Mutex;

type Completion = Box<dyn FnOnce(Result<String, AuthProblem>) + Send>;

/// How long a login page may stay unanswered.
pub const LOGIN_TIMEOUT_SECONDS: f64 = 10.0 * 60.0;

pub struct PetCallbackBrowser {
    /// Registers the link scheme for this user (HKCU). Called when the login button is pressed.
    prepare: Box<dyn Fn() + Send + Sync>,
    open: Box<dyn Fn(&str) + Send + Sync>,
    now: Box<dyn Fn() -> f64 + Send + Sync>,
    waiting: Mutex<Option<(Completion, f64)>>,
}

impl PetCallbackBrowser {
    pub fn new(prepare: Box<dyn Fn() + Send + Sync>, open: Box<dyn Fn(&str) + Send + Sync>, now: Box<dyn Fn() -> f64 + Send + Sync>) -> Self {
        Self { prepare, open, now, waiting: Mutex::new(None) }
    }

    pub fn is_waiting(&self) -> bool { self.waiting.lock().unwrap().is_some() }

    /// The address the system handed to a second copy of the app. True when a sign-in was waiting for it and took it
    /// (checks of `state` and the code come after, in the auth). Anything else is ignored.
    pub fn deliver(&self, url: &str) -> bool {
        if callback_from_arguments(std::slice::from_ref(&url.to_string())).is_none() { return false; }
        let taken = self.waiting.lock().unwrap().take();
        match taken {
            Some((done, _)) => { done(Ok(url.to_string())); true }
            None => false,
        }
    }

    /// The user closed the chat, or switched Nacho Pet off, while the login page was open.
    pub fn cancel(&self) {
        let taken = self.waiting.lock().unwrap().take();
        if let Some((done, _)) = taken { done(Err(AuthProblem::Canceled)); }
    }

    /// Ends a login that nobody finished. Returns true when it did.
    pub fn expire(&self) -> bool {
        let now = (self.now)();
        let taken = {
            let mut w = self.waiting.lock().unwrap();
            if w.as_ref().is_some_and(|(_, since)| now - *since >= LOGIN_TIMEOUT_SECONDS) { w.take() } else { None }
        };
        match taken {
            Some((done, _)) => { done(Err(AuthProblem::Canceled)); true }
            None => false,
        }
    }
}

impl PetAuthBrowser for PetCallbackBrowser {
    fn authorize(&self, url: &str, _callback_scheme: &str, completion: Completion) {
        // Only the login page's own web address may be opened in the browser.
        if !(url.starts_with("https://") || url.starts_with("http://")) { return completion(Err(AuthProblem::NotConfigured)); }
        let older = self.waiting.lock().unwrap().replace((completion, (self.now)()));
        if let Some((done, _)) = older { done(Err(AuthProblem::Canceled)); } // a second press starts over
        (self.prepare)();
        (self.open)(url);
    }
}

/// The login redirect among the command-line arguments of a copy the system started for a `nachogadget:` link.
pub fn callback_from_arguments(args: &[String]) -> Option<String> {
    let prefix = OIDCConfig::REDIRECT_URI;
    args.iter()
        .find(|a| a.len() <= 8192 && a.len() >= prefix.len() && a.is_char_boundary(prefix.len()) && a[..prefix.len()].eq_ignore_ascii_case(prefix)
            && matches!(a.as_bytes().get(prefix.len()), None | Some(b'?') | Some(b'#')))
        .cloned()
}

/// `HKCU\Software\Classes\nachogadget` and what goes in it: (subkey, value name or None for the default, value).
/// The command runs the exe that is running now, with the link as its only argument.
pub fn protocol_registration(exe: &str) -> Vec<(String, Option<&'static str>, String)> {
    let root = format!("Software\\Classes\\{}", OIDCConfig::CALLBACK_SCHEME);
    vec![
        (root.clone(), None, "URL:Nacho Gadget Login".into()),
        (root.clone(), Some("URL Protocol"), String::new()),
        (format!("{root}\\shell\\open\\command"), None, format!("\"{exe}\" \"%1\"")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<String>>>;

    fn browser() -> (Arc<PetCallbackBrowser>, Seen, Arc<Mutex<f64>>) {
        let log: Seen = Arc::default();
        let clock = Arc::new(Mutex::new(1000.0));
        let (l1, l2, c) = (log.clone(), log.clone(), clock.clone());
        let b = PetCallbackBrowser::new(Box::new(move || l1.lock().unwrap().push("register".into())), Box::new(move |u| l2.lock().unwrap().push(format!("open {u}"))), Box::new(move || *c.lock().unwrap()));
        (Arc::new(b), log, clock)
    }

    fn collect() -> (Completion, Arc<Mutex<Vec<Result<String, AuthProblem>>>>) {
        let out: Arc<Mutex<Vec<Result<String, AuthProblem>>>> = Arc::default();
        let o = out.clone();
        (Box::new(move |r| o.lock().unwrap().push(r)), out)
    }

    const CB: &str = "nachogadget://auth/callback?code=abc&state=xyz";

    #[test]
    fn the_scheme_is_registered_when_login_starts_and_not_before() {
        let (b, log, _) = browser();
        assert!(log.lock().unwrap().is_empty(), "creating the browser registers nothing");
        let (done, out) = collect();
        b.authorize("https://login.example/auth?x=1", "nachogadget", done);
        assert_eq!(*log.lock().unwrap(), vec!["register".to_string(), "open https://login.example/auth?x=1".to_string()]);
        assert!(out.lock().unwrap().is_empty() && b.is_waiting());
    }

    #[test]
    fn the_callback_reaches_the_waiting_login_once() {
        let (b, _, _) = browser();
        let (done, out) = collect();
        b.authorize("https://login.example/a", "nachogadget", done);
        assert!(b.deliver(CB));
        assert_eq!(*out.lock().unwrap(), vec![Ok(CB.to_string())]);
        assert!(!b.deliver(CB), "a second delivery has nobody to take it");
        assert!(!b.is_waiting());
    }

    #[test]
    fn addresses_nobody_asked_for_are_ignored() {
        let (b, _, _) = browser();
        assert!(!b.deliver(CB), "no login waiting");
        let (done, out) = collect();
        b.authorize("https://login.example/a", "nachogadget", done);
        for bad in ["https://evil.example/?code=1", "nachogadget://other/callback?code=1", "nachogadget://auth/callbackx?code=1", ""] {
            assert!(!b.deliver(bad), "{bad}");
        }
        assert!(b.is_waiting() && out.lock().unwrap().is_empty(), "the real login still waits");
    }

    #[test]
    fn cancel_second_press_and_timeout_end_the_waiting_login() {
        let (b, _, clock) = browser();
        let (d1, o1) = collect();
        b.authorize("https://login.example/a", "nachogadget", d1);
        let (d2, o2) = collect();
        b.authorize("https://login.example/b", "nachogadget", d2);
        assert_eq!(*o1.lock().unwrap(), vec![Err(AuthProblem::Canceled)], "the older page is dropped");
        assert!(!b.expire(), "not yet");
        *clock.lock().unwrap() += LOGIN_TIMEOUT_SECONDS;
        assert!(b.expire());
        assert_eq!(*o2.lock().unwrap(), vec![Err(AuthProblem::Canceled)]);
        let (d3, o3) = collect();
        b.authorize("https://login.example/c", "nachogadget", d3);
        b.cancel();
        assert_eq!(*o3.lock().unwrap(), vec![Err(AuthProblem::Canceled)]);
    }

    #[test]
    fn only_web_addresses_open_in_the_browser() {
        let (b, log, _) = browser();
        let (done, out) = collect();
        b.authorize("file:///C:/Windows/System32/calc.exe", "nachogadget", done);
        assert_eq!(*out.lock().unwrap(), vec![Err(AuthProblem::NotConfigured)]);
        assert!(log.lock().unwrap().is_empty(), "nothing registered, nothing opened");
    }

    #[test]
    fn the_callback_is_found_among_the_arguments_of_the_second_copy() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(callback_from_arguments(&args(&[CB])), Some(CB.to_string()));
        assert_eq!(callback_from_arguments(&args(&["--debug-ctl", "NACHOGADGET://auth/callback?code=1"])), Some("NACHOGADGET://auth/callback?code=1".into()));
        assert_eq!(callback_from_arguments(&args(&["--debug-ctl"])), None);
        assert_eq!(callback_from_arguments(&args(&["nachogadget://auth/callback/../x"])), None);
        assert_eq!(callback_from_arguments(&args(&[&format!("nachogadget://auth/callback?{}", "a".repeat(9000))])), None, "too long");
    }

    #[test]
    fn the_registration_is_per_user_and_runs_this_exe_with_the_link() {
        let r = protocol_registration("C:\\Users\\me\\NachoCombo.exe");
        assert!(r.iter().all(|(k, _, _)| k.starts_with("Software\\Classes\\nachogadget")), "HKCU only, never a machine key");
        assert!(r.iter().any(|(k, n, v)| k.ends_with("shell\\open\\command") && n.is_none() && v == "\"C:\\Users\\me\\NachoCombo.exe\" \"%1\""));
        assert!(r.iter().any(|(_, n, _)| *n == Some("URL Protocol")));
    }
}
