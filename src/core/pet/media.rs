//! Pictures and files handed to the pet (Capture Float → pet, File Shelf → pet, drops on the pet). They are uploaded with
//! `POST /api/v1/media/uploads` (JSON form, nacho-server spec/media.tsp) and sent as `attachments: [{attachment_id}]`.
//! What the model can see is decided by the server: jpeg/png/webp/gif up to 5 MB are shown to Nacho, txt/md/csv are
//! read as text, everything else is only stored. Port of `PetMedia.swift`.
use super::http::{PetHTTPClient, PetHTTPRequest, PetServerError};
use super::json::{self, JsonExt};
use super::settings::ServerEndpoint;
use serde_json::json;

/// An uploaded attachment, as the server returned it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetAttachment {
    pub attachment_id: String,
    pub kind: String,
    pub name: String,
    pub mime: String,
    pub bytes: i64,
}

/// A file waiting to be uploaded with the next message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetOutgoingFile {
    pub name: String,
    pub mime: String,
    pub kind: String,
    pub data: Vec<u8>,
}

impl PetOutgoingFile {
    pub fn readability(&self) -> Readability { PetMedia::readability(&self.mime, self.data.len()) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaProblem { Empty, TooLarge, UnsupportedType, TooMany, Unauthorized, Server(u16), Network, BadResponse }

impl MediaProblem {
    pub fn korean_message(&self) -> String {
        match self {
            Self::Empty => "빈 파일은 보낼 수 없어요.".into(),
            Self::TooLarge => format!("파일이 너무 커요. 한 파일은 {}MB까지예요.", PetMedia::MAX_UPLOAD_BYTES / 1_000_000),
            Self::UnsupportedType => "이 파일 형식은 보낼 수 없어요. 사진·PDF·텍스트(txt·md·csv)·mp4만 보낼 수 있어요.".into(),
            Self::TooMany => format!("한 번에 붙일 수 있는 파일은 {}개까지예요.", PetMedia::MAX_ATTACHMENTS),
            Self::Unauthorized => "로그인이 필요해요.".into(),
            Self::Server(code) => format!("파일을 올리지 못했어요. (HTTP {code})"),
            Self::Network => "파일을 올리지 못했어요. 네트워크를 확인해 주세요.".into(),
            Self::BadResponse => "서버 응답을 읽지 못했어요.".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readability {
    /// Nacho sees the picture.
    Seen,
    /// Nacho reads the beginning of the text.
    Read,
    /// Stored with the message; Nacho is told it cannot open it.
    StoredOnly,
}

impl Readability {
    pub fn korean_note(&self) -> &'static str {
        match self {
            Self::Seen => "나초가 사진을 볼 수 있어요",
            Self::Read => "나초가 글을 읽을 수 있어요",
            Self::StoredOnly => "나초는 내용을 열어 보지 못해요 (전달만 돼요)",
        }
    }
}

pub struct PetMedia;

impl PetMedia {
    pub const MAX_ATTACHMENTS: usize = 4;
    /// Server default `media.max_bytes` (20 MB); the server answers 413 above it.
    pub const MAX_UPLOAD_BYTES: usize = 20_000_000;
    /// The server shows a picture to the model only up to this size (`MAX_IMAGE_BYTES`).
    pub const MAX_SEEN_IMAGE_BYTES: usize = 5 * 1024 * 1024;

    /// Closed table: extension → mime. Anything else is refused before upload.
    pub const MIME_BY_EXTENSION: &'static [(&'static str, &'static str)] = &[
        ("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp"),
        ("heic", "image/heic"), ("pdf", "application/pdf"), ("txt", "text/plain"), ("md", "text/markdown"),
        ("markdown", "text/markdown"), ("csv", "text/csv"), ("mp4", "video/mp4"),
    ];

    pub fn mime_for_file_name(name: &str) -> Option<&'static str> {
        let ext = Self::file_extension(name).to_lowercase();
        Self::MIME_BY_EXTENSION.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
    }

    /// The server's allow-list: image/*, video/mp4 (exactly), application/pdf, text/*.
    pub fn is_allowed(mime: &str) -> bool {
        let m = mime.to_lowercase();
        m.starts_with("image/") || m == "video/mp4" || m == "application/pdf" || m.starts_with("text/")
    }

    pub fn kind_for_mime(mime: &str) -> &'static str {
        let m = mime.to_lowercase();
        if m.starts_with("image/") { "image" } else if m == "video/mp4" { "video" } else { "file" }
    }

    pub fn readability(mime: &str, bytes: usize) -> Readability {
        match mime.to_lowercase().as_str() {
            "image/jpeg" | "image/png" | "image/webp" | "image/gif" => {
                if bytes <= Self::MAX_SEEN_IMAGE_BYTES { Readability::Seen } else { Readability::StoredOnly }
            }
            "text/plain" | "text/markdown" | "text/csv" => Readability::Read,
            _ => Readability::StoredOnly,
        }
    }

    /// True when a picture should be made smaller before upload so that Nacho can see it (captures of big screens).
    pub fn should_shrink_for_nacho(mime: &str, bytes: usize) -> bool {
        mime.to_lowercase().starts_with("image/") && bytes > Self::MAX_SEEN_IMAGE_BYTES
    }

    /// File name shown to the server: last path component (`/` or `\`), control characters removed, ≤ 120 characters.
    pub fn sanitized_name(raw: &str) -> String {
        let last = raw.split(['/', '\\']).rev().find(|s| !s.is_empty()).unwrap_or("");
        let clean: String = last.chars().filter(|c| (*c as u32) >= 0x20 && *c != '\u{7f}').collect();
        let clean = clean.trim();
        let name = if clean.is_empty() || clean == "." || clean == ".." { "file" } else { clean };
        if name.chars().count() <= 120 { return name.to_string(); }
        let ext = Self::file_extension(name);
        let keep = 120 - if ext.is_empty() { 0 } else { ext.chars().count() + 1 };
        let head: String = name.chars().take(keep).collect();
        if ext.is_empty() { head } else { format!("{head}.{ext}") }
    }

    /// Text after the last dot of the last path component ("" when none, or for names like ".profile").
    pub fn file_extension(name: &str) -> String {
        let last = name.split(['/', '\\']).rev().find(|s| !s.is_empty()).unwrap_or(name);
        match last.rfind('.') {
            Some(0) | None => String::new(),
            Some(i) => last[i + 1..].to_string(),
        }
    }

    /// Validates a file before it may be attached. `mime` None = derived from the file name.
    pub fn prepare(name: &str, data: Vec<u8>, mime: Option<&str>) -> Result<PetOutgoingFile, MediaProblem> {
        let clean = Self::sanitized_name(name);
        let m = mime.map(str::to_string).or_else(|| Self::mime_for_file_name(&clean).map(str::to_string)).map(|m| m.to_lowercase());
        let Some(m) = m.filter(|m| Self::is_allowed(m)) else { return Err(MediaProblem::UnsupportedType) };
        if data.is_empty() { return Err(MediaProblem::Empty); }
        if data.len() > Self::MAX_UPLOAD_BYTES { return Err(MediaProblem::TooLarge); }
        Ok(PetOutgoingFile { name: clean, kind: Self::kind_for_mime(&m).into(), mime: m, data })
    }

    pub fn upload_request(endpoint: &ServerEndpoint, bearer: Option<&str>, file: &PetOutgoingFile) -> PetHTTPRequest {
        PetHTTPRequest::post_json(
            endpoint.rest("/api/v1/media/uploads"),
            bearer,
            &json!({"kind": file.kind, "name": file.name, "mime": file.mime, "bytes_base64": super::crypto::base64(&file.data)}),
        )
    }

    pub fn decode_upload(data: &[u8]) -> Option<PetAttachment> {
        let o = json::object(data)?;
        let id = o.str("attachment_id").filter(|id| PetIDs::is_uuid(id))?;
        Some(PetAttachment {
            attachment_id: id,
            kind: o.str("kind").unwrap_or_else(|| "file".into()),
            name: o.str("name").unwrap_or_default(),
            mime: o.str("mime").unwrap_or_default(),
            bytes: o.int("bytes").unwrap_or(0),
        })
    }

    /// The upload answer → attachment or problem.
    pub fn upload_result(result: super::http::HTTPResult) -> Result<PetAttachment, MediaProblem> {
        let resp = result.map_err(|_| MediaProblem::Network)?;
        match resp.status {
            200..=299 => Self::decode_upload(&resp.body).ok_or(MediaProblem::BadResponse),
            401 | 403 => Err(MediaProblem::Unauthorized),
            413 => Err(MediaProblem::TooLarge),
            400 if PetServerError::code(&resp.body).as_deref() == Some("validation_error") => Err(MediaProblem::UnsupportedType),
            s => Err(MediaProblem::Server(s)),
        }
    }

    pub fn upload(
        file: &PetOutgoingFile,
        endpoint: &ServerEndpoint,
        bearer: Option<&str>,
        http: &dyn PetHTTPClient,
        completion: Box<dyn FnOnce(Result<PetAttachment, MediaProblem>) + Send>,
    ) {
        http.send(Self::upload_request(endpoint, bearer, file), Box::new(move |r| completion(Self::upload_result(r))));
    }
}

pub struct PetIDs;

impl PetIDs {
    /// 8-4-4-4-12 hex (any version).
    pub fn is_uuid(s: &str) -> bool {
        let parts: Vec<&str> = s.split('-').collect();
        parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    /// A random version-4 UUID (lowercase) for ids the pet makes up (client message ids, missing update ids). Not for
    /// secrets: PKCE verifiers and `state` take their randomness from the OS through the shell.
    pub fn new_uuid() -> String {
        use std::hash::{BuildHasher, Hasher};
        let mut bytes = [0u8; 16];
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
            h.write_usize(i);
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
        Self::uuid_from_bytes(bytes)
    }

    /// Formats 16 bytes as a version-4 / variant-1 UUID.
    pub fn uuid_from_bytes(mut b: [u8; 16]) -> String {
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
        format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::http::PetHTTPResponse;

    #[test]
    fn prepare_checks_type_size_and_name() {
        let f = PetMedia::prepare("C:\\Users\\me\\캡처 1.PNG", vec![1, 2, 3], None).unwrap();
        assert_eq!((f.name.as_str(), f.mime.as_str(), f.kind.as_str()), ("캡처 1.PNG", "image/png", "image"));
        assert_eq!(f.readability(), Readability::Seen);
        assert_eq!(PetMedia::prepare("run.sh", vec![1], None), Err(MediaProblem::UnsupportedType));
        assert_eq!(PetMedia::prepare("a.txt", vec![], None), Err(MediaProblem::Empty));
        assert_eq!(PetMedia::prepare("a.txt", vec![0; PetMedia::MAX_UPLOAD_BYTES + 1], None), Err(MediaProblem::TooLarge));
        assert_eq!(PetMedia::prepare("clip", vec![1], Some("TEXT/Plain")).unwrap().mime, "text/plain");
        assert_eq!(PetMedia::prepare("x.mov", vec![1], Some("video/quicktime")), Err(MediaProblem::UnsupportedType));
        assert_eq!(PetMedia::sanitized_name("a/b/\u{7}x\n.md "), "x.md");
        assert_eq!(PetMedia::sanitized_name(".."), "file");
        let long = format!("{}.md", "가".repeat(200));
        let s = PetMedia::sanitized_name(&long);
        assert_eq!(s.chars().count(), 120);
        assert!(s.ends_with(".md"));
        assert_eq!(PetMedia::file_extension(".profile"), "");
        assert_eq!(PetMedia::readability("image/png", PetMedia::MAX_SEEN_IMAGE_BYTES + 1), Readability::StoredOnly);
        assert!(PetMedia::should_shrink_for_nacho("image/png", PetMedia::MAX_SEEN_IMAGE_BYTES + 1));
        assert_eq!(PetMedia::readability("application/pdf", 10), Readability::StoredOnly);
    }

    #[test]
    fn upload_request_and_errors() {
        let ep = ServerEndpoint::parse("https://nacho.example").unwrap();
        let f = PetMedia::prepare("a.txt", b"hi".to_vec(), None).unwrap();
        let r = PetMedia::upload_request(&ep, Some("t"), &f);
        assert_eq!(r.url, "https://nacho.example/api/v1/media/uploads");
        assert_eq!(String::from_utf8(r.body.unwrap()).unwrap(), r#"{"bytes_base64":"aGk=","kind":"file","mime":"text/plain","name":"a.txt"}"#);
        let id = "0b9f3c1e-4a2d-4f6b-8c7d-112233445566";
        let ok = PetMedia::upload_result(Ok(PetHTTPResponse::new(201, format!(r#"{{"attachment_id":"{id}","kind":"file","bytes":2}}"#))));
        assert_eq!(ok.unwrap().attachment_id, id);
        assert_eq!(PetMedia::upload_result(Ok(PetHTTPResponse::new(200, r#"{"attachment_id":"nope"}"#))), Err(MediaProblem::BadResponse));
        assert_eq!(PetMedia::upload_result(Ok(PetHTTPResponse::new(401, ""))), Err(MediaProblem::Unauthorized));
        assert_eq!(PetMedia::upload_result(Ok(PetHTTPResponse::new(413, ""))), Err(MediaProblem::TooLarge));
        assert_eq!(PetMedia::upload_result(Ok(PetHTTPResponse::new(400, r#"{"error":{"code":"validation_error"}}"#))), Err(MediaProblem::UnsupportedType));
        assert_eq!(PetMedia::upload_result(Ok(PetHTTPResponse::new(500, ""))), Err(MediaProblem::Server(500)));
        assert_eq!(PetMedia::upload_result(Err(crate::core::pet::http::PetNetError("x".into()))), Err(MediaProblem::Network));
    }

    #[test]
    fn uuids() {
        let a = PetIDs::new_uuid();
        assert!(PetIDs::is_uuid(&a));
        assert_ne!(a, PetIDs::new_uuid());
        assert_eq!(&a[14..15], "4");
        assert!(!PetIDs::is_uuid("0b9f3c1e-4a2d-4f6b-8c7d-11223344556g"));
        assert_eq!(PetIDs::uuid_from_bytes([0; 16]), "00000000-0000-4000-8000-000000000000");
    }
}
