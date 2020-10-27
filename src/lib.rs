extern crate flatbuffers;

use std::io;
use std::time::SystemTime;

use flatbuffers::FlatBufferBuilder;
use rocket::State;
use rocksdb::{compaction_filter, DB};

#[path = "api_generated.rs"]
mod api_generated;
use crate::api_generated::api::{finish_entry_buffer, get_root_as_entry, Entry, EntryArgs};

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

    let entry = get_root_as_entry(value);
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

pub fn get_extension(filename: &str) -> &str {
    filename
        .rfind('.')
        .map(|idx| &filename[idx..])
        .filter(|ext| ext.chars().skip(1).all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("")
}

pub fn get_entry_data<'r>(id: &str, state: &'r State<'r, DB>) -> Result<Vec<u8>, io::Error> {
    // read data from DB to Entry struct
    let root = match state.get(id).unwrap() {
        Some(root) => root,
        None => return Err(io::Error::new(io::ErrorKind::NotFound, "record not found")),
    };
    let entry = get_root_as_entry(&root);

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

pub fn new_entry(
    dest: &mut Vec<u8>,
    data: &mut rocket::data::DataStream,
    lang: String,
    ttl: u64,
    burn: bool,
    encrypted: bool,
) {
    let mut bldr = FlatBufferBuilder::new();

    dest.clear();
    bldr.reset();

    // potential speed improvement over the create_vector:
    // https://docs.rs/flatbuffers/0.6.1/flatbuffers/struct.FlatBufferBuilder.html#method.create_vector
    let mut tmp_vec: Vec<u8> = vec![];
    std::io::copy(data, &mut tmp_vec).unwrap();

    bldr.start_vector::<u8>(tmp_vec.len());
    for byte in tmp_vec.iter().rev() {
        bldr.push::<u8>(*byte);
    }
    let data_vec = bldr.end_vector::<u8>(tmp_vec.len());

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
        lang: Some(bldr.create_string(&lang)),
        burn,
        encrypted,
    };

    let user_offset = Entry::create(&mut bldr, &args);
    finish_entry_buffer(&mut bldr, user_offset);

    let finished_data = bldr.finished_data();
    dest.extend_from_slice(finished_data);
}
