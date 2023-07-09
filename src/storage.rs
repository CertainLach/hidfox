use std::collections::BTreeMap;

use bifrostlink::request;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{Address, NoopResponse, Rpc};

#[derive(Serialize, Deserialize)]
struct StorageGet {
	key: String,
}
request!(StorageGet => StorageGetR);
#[derive(Serialize, Deserialize)]
struct StorageGetR {
	value: Option<String>,
}

#[derive(Serialize)]
struct StorageRemove {
	key: String,
}
request!(StorageRemove => NoopResponse);

#[derive(Serialize)]
struct StorageSet {
	key: String,
	value: String,
	expires_in: Option<u32>,
}
request!(StorageSet => NoopResponse);

#[derive(Serialize)]
struct StorageGetPrefix {
	prefix: String,
}
#[derive(Deserialize)]
struct StorageItem {
	key: String,
	value: String,
}
type StorageGetPrefixR = Vec<StorageItem>;
request!(StorageGetPrefix => StorageGetPrefixR);

pub(crate) async fn storage_get_prefix<K: DeserializeOwned + Ord, T: DeserializeOwned>(
	r: &Rpc,
	prefix: &str,
) -> BTreeMap<K, T> {
	let result = match r
		.request(
			Address::Background,
			&StorageGetPrefix {
				prefix: prefix.to_owned(),
			},
		)
		.await
	{
		Ok(v) => v,
		Err(e) => {
			eprintln!("storage prefix request failed: {e:?}");
			vec![]
		}
	};

	let mut out = BTreeMap::new();

	for i in result {
		let key = match serde_json::from_str(&i.key) {
			Ok(v) => v,
			Err(e) => {
				eprintln!("failed to decode key: {}\n{e}", i.key);
				continue;
			}
		};
		let value = match serde_json::from_str(&i.value) {
			Ok(v) => v,
			Err(e) => {
				eprintln!("failed to decode key: {}\n{e}", i.value);
				continue;
			}
		};
		out.insert(key, value);
	}

	out
}
pub(crate) async fn storage_get<T: DeserializeOwned, K: Serialize>(
	r: &Rpc,
	prefix: &str,
	key: K,
) -> Option<T> {
	let key = format_key(prefix, key);

	let result = match r
		.request(
			Address::Background,
			&StorageGet {
				key: key.to_owned(),
			},
		)
		.await
	{
		Ok(v) => v,
		Err(e) => {
			eprintln!("storage request failed: {e:?}");
			return None;
		}
	};

	let value = result.value?;

	match serde_json::from_str(&value) {
		Ok(v) => return Some(v),
		Err(e) => {
			eprintln!("storage decode failed: {e}");
			return None;
		}
	}
}

pub(crate) async fn storage_remove<K: Serialize>(r: &mut Rpc, prefix: &str, key: K) {
	let key = format_key(prefix, key);

	let _ = r
		.request(
			Address::Background,
			&StorageRemove {
				key: key.to_owned(),
			},
		)
		.await;
}

pub(crate) async fn storage_set<K: Serialize, T: Serialize>(
	r: &Rpc,
	prefix: &str,
	key: K,
	value: T,
	expires_in: Option<u32>,
) {
	let key = format_key(prefix, key);
	let value = serde_json::to_string(&value).expect("serialize failed");

	let _ = r
		.request(
			Address::Background,
			&StorageSet {
				key,
				value,
				expires_in,
			},
		)
		.await;
}

fn format_key<K: Serialize>(prefix: &str, key: K) -> String {
	let mut full_key = prefix.to_owned();
	full_key.push_str(&serde_json::to_string(&key).unwrap());
	full_key
}
