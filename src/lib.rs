extern crate flatbuffers;

use std::io;
use std::time::SystemTime;

use flatbuffers::FlatBufferBuilder;
use rocket::State;
use rocksdb::{compaction_filter, DB};

#[path = "api_generated.rs"]
mod api_generated;
use crate::api_generated::api::{finish_entry_buffer, root_as_entry, Entry, EntryArgs};

#[macro_export]
macro_rules! load_static_resources(
    { $($key:expr => $value:expr),+ } => {
        {
            let mut resources: HashMap<&'static str, &'static [u8]> = HashMap::new();
            $(
                resources.insert($key, include_bytes!($value));
            )*

            resources
        }
     };
);

pub fn compaction_filter_expired_entries(
    _: u32,
    _: &[u8],
    value: &[u8],
) -> compaction_filter::Decision {
    use compaction_filter::Decision::*;

    let entry = root_as_entry(value).unwrap();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("time went backwards")
        .as_secs();

    if entry.expiry_timestamp() != 0 && now >= entry.expiry_timestamp() {
        Remove
    } else {
        Keep
    }
}

/// Validate a Prism language identifier. Prism names are lowercase alphanumeric
/// plus `-`, `_`, `+`, `#` (e.g. "c++", "c#", "shell-session"). Anything else
/// falls back to "markup" so it can never inject arbitrary CSS class names.
pub fn sanitize_lang(lang: &str) -> &str {
    if !lang.is_empty()
        && lang
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '#'))
    {
        lang
    } else {
        "markup"
    }
}

pub fn get_extension(filename: &str) -> &str {
    filename
        .rfind('.')
        .map(|idx| &filename[idx..])
        .filter(|ext| ext.chars().skip(1).all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("")
}

pub fn get_entry_data(id: &str, state: &State<DB>) -> Result<Vec<u8>, io::Error> {
    // read data from DB to Entry struct
    let root = match state.get(id).unwrap() {
        Some(root) => root,
        None => return Err(io::Error::new(io::ErrorKind::NotFound, "record not found")),
    };
    let entry = root_as_entry(&root).unwrap();

    // check if data expired (might not be yet deleted by rocksb compaction hook)
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("time went backwards")
        .as_secs();

    if entry.expiry_timestamp() != 0 && now >= entry.expiry_timestamp() {
        state.delete(id).unwrap();
        return Err(io::Error::new(io::ErrorKind::NotFound, "record not found"));
    }

    // "burn" one time only pastebin content
    if entry.burn() {
        state.delete(id).unwrap();
    }

    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rocksdb::compaction_filter::Decision;

    // ── sanitize_lang ─────────────────────────────────────────────────────────

    #[test]
    fn sanitize_lang_accepts_valid_identifiers() {
        assert_eq!(sanitize_lang("javascript"), "javascript");
        assert_eq!(sanitize_lang("c++"), "c++");
        assert_eq!(sanitize_lang("c#"), "c#");
        assert_eq!(sanitize_lang("shell-session"), "shell-session");
        assert_eq!(sanitize_lang("markup"), "markup");
    }

    #[test]
    fn sanitize_lang_rejects_invalid_chars() {
        assert_eq!(sanitize_lang("java script"), "markup"); // space
        assert_eq!(sanitize_lang("<script>"), "markup");    // angle brackets
        assert_eq!(sanitize_lang("lang/../../etc"), "markup"); // path traversal chars
        assert_eq!(sanitize_lang(""), "markup");            // empty string
    }

    // ── get_extension ─────────────────────────────────────────────────────────

    #[test]
    fn get_extension_standard() {
        assert_eq!(get_extension("file.js"), ".js");
        assert_eq!(get_extension("archive.tar.gz"), ".gz");
        assert_eq!(get_extension("image.PNG"), ".PNG");
    }

    #[test]
    fn get_extension_no_dot_returns_empty() {
        assert_eq!(get_extension("Makefile"), "");
        assert_eq!(get_extension(""), "");
    }

    #[test]
    fn get_extension_non_alphanumeric_chars_returns_empty() {
        // hyphen in extension is not alphanumeric → filtered out
        assert_eq!(get_extension("file.tar-gz"), "");
        assert_eq!(get_extension("file.tar.gz-backup"), "");
    }

    // ── compaction_filter_expired_entries ──────────────────────────────────────

    fn make_entry_with_expiry(expiry_timestamp: u64) -> Vec<u8> {
        use api_generated::api::{finish_entry_buffer, Entry, EntryArgs};
        use flatbuffers::FlatBufferBuilder;

        let mut bldr = FlatBufferBuilder::new();
        let data = bldr.create_vector(b"test");
        let lang = bldr.create_string("text");
        let args = EntryArgs {
            create_timestamp: 0,
            expiry_timestamp,
            data: Some(data),
            lang: Some(lang),
            burn: false,
            encrypted: false,
        };
        let offset = Entry::create(&mut bldr, &args);
        finish_entry_buffer(&mut bldr, offset);
        bldr.finished_data().to_vec()
    }

    #[test]
    fn compaction_filter_keeps_entry_without_expiry() {
        let buf = make_entry_with_expiry(0);
        assert!(matches!(compaction_filter_expired_entries(0, &[], &buf), Decision::Keep));
    }

    #[test]
    fn compaction_filter_keeps_entry_with_future_expiry() {
        let far_future = u32::MAX as u64; // year 2106
        let buf = make_entry_with_expiry(far_future);
        assert!(matches!(compaction_filter_expired_entries(0, &[], &buf), Decision::Keep));
    }

    #[test]
    fn compaction_filter_removes_entry_with_past_expiry() {
        let buf = make_entry_with_expiry(1); // Unix epoch + 1s — definitely in the past
        assert!(matches!(compaction_filter_expired_entries(0, &[], &buf), Decision::Remove));
    }
}

pub fn new_entry(
    dest: &mut Vec<u8>,
    data: &[u8],
    lang: &str,
    ttl: u64,
    burn: bool,
    encrypted: bool,
) {
    let mut bldr = FlatBufferBuilder::new();

    dest.clear();
    bldr.reset();

    let data_vec = bldr.create_vector(data);

    // calc expiry datetime
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("time went backwards")
        .as_secs();
    let expiry = if ttl == 0 { ttl } else { now + ttl };

    // setup actual struct
    let args = EntryArgs {
        create_timestamp: now,
        expiry_timestamp: expiry,
        data: Some(data_vec),
        lang: Some(bldr.create_string(lang)),
        burn,
        encrypted,
    };

    let user_offset = Entry::create(&mut bldr, &args);
    finish_entry_buffer(&mut bldr, user_offset);

    let finished_data = bldr.finished_data();
    dest.extend_from_slice(finished_data);
}
