#[cfg(not(feature = "std"))]
use sr_std::prelude::*;
use codec::{Codec, Encode, EncodeAppend};
use crate::{storage::{self, unhashed, hashed::StorageHasher}, rstd::borrow::Borrow};

pub trait StorageMap<K: Codec, V: Codec> {
	/// The type that get/take returns.
	type Query;

	type Hasher: StorageHasher;

	fn prefix() -> &'static [u8];

	fn from_optional_value_to_query(v: Option<V>) -> Self::Query;

	fn from_query_to_optional_value(v: Self::Query) -> Option<V>;

	fn storage_map_final_key<KeyArg>(key: KeyArg) -> <Self::Hasher as StorageHasher>::Output
	where
		KeyArg: Borrow<K>,
	{
		let mut final_key = Self::prefix().to_vec();
		key.borrow().encode_to(&mut final_key);
		Self::Hasher::hash(&final_key)
	}
}

impl<K: Codec, V: Codec, G: StorageMap<K, V>> storage::StorageMap<K, V> for G {
	type Query = G::Query;

	fn key_for<KeyArg: Borrow<K>>(key: KeyArg) -> Vec<u8> {
		Self::storage_map_final_key(key).as_ref().to_vec()
	}

	fn swap<KeyArg1: Borrow<K>, KeyArg2: Borrow<K>>(key1: KeyArg1, key2: KeyArg2) {
		let k1 = Self::storage_map_final_key(key1);
		let k2 = Self::storage_map_final_key(key2);

		let v1 = unhashed::get_raw(k1.as_ref());
		if let Some(val) = unhashed::get_raw(k2.as_ref()) {
			unhashed::put_raw(k1.as_ref(), &val);
		} else {
			unhashed::kill(k1.as_ref())
		}
		if let Some(val) = v1 {
			unhashed::put_raw(k2.as_ref(), &val);
		} else {
			unhashed::kill(k2.as_ref())
		}
	}

	fn exists<KeyArg: Borrow<K>>(key: KeyArg) -> bool {
		unhashed::exists(Self::storage_map_final_key(key).as_ref())
	}

	fn get<KeyArg: Borrow<K>>(key: KeyArg) -> Self::Query {
		G::from_optional_value_to_query(unhashed::get(Self::storage_map_final_key(key).as_ref()))
	}

	fn insert<KeyArg: Borrow<K>, ValArg: Borrow<V>>(key: KeyArg, val: ValArg) {
		unhashed::put(Self::storage_map_final_key(key).as_ref(), &val.borrow())
	}

	fn insert_ref<KeyArg: Borrow<K>, ValArg: ?Sized + Encode>(key: KeyArg, val: &ValArg)
		where V: AsRef<ValArg>
	{
		val.using_encoded(|b| unhashed::put_raw(Self::storage_map_final_key(key).as_ref(), b))
	}

	fn remove<KeyArg: Borrow<K>>(key: KeyArg) {
		unhashed::kill(Self::storage_map_final_key(key).as_ref())
	}

	fn mutate<KeyArg: Borrow<K>, R, F: FnOnce(&mut Self::Query) -> R>(key: KeyArg, f: F) -> R {
		let mut val = G::get(key.borrow());

		let ret = f(&mut val);
		match G::from_query_to_optional_value(val) {
			Some(ref val) => G::insert(key, val),
			None => G::remove(key),
		}
		ret
	}

	fn take<KeyArg: Borrow<K>>(key: KeyArg) -> Self::Query {
		let key = Self::storage_map_final_key(key);
		let value = unhashed::get(key.as_ref());
		if value.is_some() {
			unhashed::kill(key.as_ref())
		}
		G::from_optional_value_to_query(value)
	}

	fn append<KeyArg: Borrow<K>, I: Encode>(key: KeyArg, items: &[I]) -> Result<(), &'static str>
		where V: EncodeAppend<Item=I>
	{
		let key = Self::storage_map_final_key(key);
		let encoded_value = unhashed::get_raw(key.as_ref())
			.unwrap_or_else(|| {
				match G::from_query_to_optional_value(G::from_optional_value_to_query(None)) {
					Some(value) => value.encode(),
					None => vec![],
				}
			});

		let new_val = V::append(
			encoded_value,
			items,
		).map_err(|_| "Could not append given item")?;
		unhashed::put_raw(key.as_ref(), &new_val);
		Ok(())
	}
}
