// Copyright 2017-2019 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use std::{sync::Arc, cmp::Ord, panic::UnwindSafe, result, cell::RefCell, rc::Rc};
use codec::{Encode, Decode};
use sr_primitives::{
	generic::BlockId, traits::Block as BlockT, traits::NumberFor,
};
use state_machine::{
	self, OverlayedChanges, Ext, ExecutionManager, StateMachine, ExecutionStrategy,
	backend::Backend as _, ChangesTrieTransaction, StorageProof,
};
use executor::{RuntimeVersion, RuntimeInfo, NativeVersion};
use hash_db::Hasher;
use primitives::{
	offchain::OffchainExt, H256, Blake2Hasher, NativeOrEncoded, NeverNativeValue,
	traits::{CodeExecutor, KeystoreExt},
};

use crate::runtime_api::{ProofRecorder, InitializeBlock};
use crate::backend;
use crate::error;

/// Method call executor.
pub trait CallExecutor<B, H>
where
	B: BlockT,
	H: Hasher<Out=B::Hash>,
	H::Out: Ord,
{
	/// Externalities error type.
	type Error: state_machine::Error;

	/// Execute a call to a contract on top of state in a block of given hash.
	///
	/// No changes are made.
	fn call(
		&self,
		id: &BlockId<B>,
		method: &str,
		call_data: &[u8],
		strategy: ExecutionStrategy,
		side_effects_handler: Option<OffchainExt>,
	) -> Result<Vec<u8>, error::Error>;

	/// Execute a contextual call on top of state in a block of a given hash.
	///
	/// No changes are made.
	/// Before executing the method, passed header is installed as the current header
	/// of the execution context.
	fn contextual_call<
		'a,
		IB: Fn() -> error::Result<()>,
		EM: Fn(
			Result<NativeOrEncoded<R>, Self::Error>,
			Result<NativeOrEncoded<R>, Self::Error>
		) -> Result<NativeOrEncoded<R>, Self::Error>,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
	>(
		&self,
		initialize_block_fn: IB,
		at: &BlockId<B>,
		method: &str,
		call_data: &[u8],
		changes: &RefCell<OverlayedChanges>,
		initialize_block: InitializeBlock<'a, B>,
		execution_manager: ExecutionManager<EM>,
		native_call: Option<NC>,
		side_effects_handler: Option<OffchainExt>,
		proof_recorder: &Option<Rc<RefCell<ProofRecorder<B>>>>,
		enable_keystore: bool,
	) -> error::Result<NativeOrEncoded<R>> where ExecutionManager<EM>: Clone;

	/// Extract RuntimeVersion of given block
	///
	/// No changes are made.
	fn runtime_version(&self, id: &BlockId<B>) -> Result<RuntimeVersion, error::Error>;

	/// Execute a call to a contract on top of given state.
	///
	/// No changes are made.
	fn call_at_state<
		S: state_machine::Backend<H>,
		F: FnOnce(
			Result<NativeOrEncoded<R>, Self::Error>,
			Result<NativeOrEncoded<R>, Self::Error>,
		) -> Result<NativeOrEncoded<R>, Self::Error>,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
	>(&self,
		state: &S,
		overlay: &mut OverlayedChanges,
		method: &str,
		call_data: &[u8],
		manager: ExecutionManager<F>,
		native_call: Option<NC>,
		side_effects_handler: Option<OffchainExt>,
	) -> Result<
		(
			NativeOrEncoded<R>,
			(S::Transaction, H::Out),
			Option<ChangesTrieTransaction<Blake2Hasher, NumberFor<B>>>
		),
		error::Error,
	>;

	/// Execute a call to a contract on top of given state, gathering execution proof.
	///
	/// No changes are made.
	fn prove_at_state<S: state_machine::Backend<H>>(
		&self,
		mut state: S,
		overlay: &mut OverlayedChanges,
		method: &str,
		call_data: &[u8]
	) -> Result<(Vec<u8>, StorageProof), error::Error> {
		let trie_state = state.as_trie_backend()
			.ok_or_else(||
				Box::new(state_machine::ExecutionError::UnableToGenerateProof)
					as Box<dyn state_machine::Error>
			)?;
		self.prove_at_trie_state(trie_state, overlay, method, call_data)
	}

	/// Execute a call to a contract on top of given trie state, gathering execution proof.
	///
	/// No changes are made.
	fn prove_at_trie_state<S: state_machine::TrieBackendStorage<H>>(
		&self,
		trie_state: &state_machine::TrieBackend<S, H>,
		overlay: &mut OverlayedChanges,
		method: &str,
		call_data: &[u8]
	) -> Result<(Vec<u8>, StorageProof), error::Error>;

	/// Get runtime version if supported.
	fn native_runtime_version(&self) -> Option<&NativeVersion>;
}

/// Call executor that executes methods locally, querying all required
/// data from local backend.
pub struct LocalCallExecutor<B, E> {
	backend: Arc<B>,
	executor: E,
	keystore: Option<primitives::traits::BareCryptoStorePtr>,
}

impl<B, E> LocalCallExecutor<B, E> {
	/// Creates new instance of local call executor.
	pub fn new(
		backend: Arc<B>,
		executor: E,
		keystore: Option<primitives::traits::BareCryptoStorePtr>,
	) -> Self {
		LocalCallExecutor {
			backend,
			executor,
			keystore,
		}
	}
}

impl<B, E> Clone for LocalCallExecutor<B, E> where E: Clone {
	fn clone(&self) -> Self {
		LocalCallExecutor {
			backend: self.backend.clone(),
			executor: self.executor.clone(),
			keystore: self.keystore.clone(),
		}
	}
}

impl<B, E, Block> CallExecutor<Block, Blake2Hasher> for LocalCallExecutor<B, E>
where
	B: backend::Backend<Block, Blake2Hasher>,
	E: CodeExecutor + RuntimeInfo,
	Block: BlockT<Hash=H256>,
{
	type Error = E::Error;

	fn call(
		&self,
		id: &BlockId<Block>,
		method: &str,
		call_data: &[u8],
		strategy: ExecutionStrategy,
		side_effects_handler: Option<OffchainExt>,
	) -> error::Result<Vec<u8>> {
		let mut changes = OverlayedChanges::default();
		let state = self.backend.state_at(*id)?;
		let return_data = StateMachine::new(
			&state,
			self.backend.changes_trie_storage(),
			side_effects_handler,
			&mut changes,
			&self.executor,
			method,
			call_data,
			self.keystore.clone().map(KeystoreExt),
		).execute_using_consensus_failure_handler::<_, NeverNativeValue, fn() -> _>(
			strategy.get_manager(),
			false,
			None,
		)
		.map(|(result, _, _)| result)?;
		self.backend.destroy_state(state)?;
		Ok(return_data.into_encoded())
	}

	fn contextual_call<
		'a,
		IB: Fn() -> error::Result<()>,
		EM: Fn(
			Result<NativeOrEncoded<R>, Self::Error>,
			Result<NativeOrEncoded<R>, Self::Error>
		) -> Result<NativeOrEncoded<R>, Self::Error>,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
	>(
		&self,
		initialize_block_fn: IB,
		at: &BlockId<Block>,
		method: &str,
		call_data: &[u8],
		changes: &RefCell<OverlayedChanges>,
		initialize_block: InitializeBlock<'a, Block>,
		execution_manager: ExecutionManager<EM>,
		native_call: Option<NC>,
		side_effects_handler: Option<OffchainExt>,
		recorder: &Option<Rc<RefCell<ProofRecorder<Block>>>>,
		enable_keystore: bool,
	) -> Result<NativeOrEncoded<R>, error::Error> where ExecutionManager<EM>: Clone {
		match initialize_block {
			InitializeBlock::Do(ref init_block)
				if init_block.borrow().as_ref().map(|id| id != at).unwrap_or(true) => {
				initialize_block_fn()?;
			},
			// We don't need to initialize the runtime at a block.
			_ => {},
		}

		let keystore = if enable_keystore {
			self.keystore.clone().map(KeystoreExt)
		} else {
			None
		};

		let mut state = self.backend.state_at(*at)?;

		let result = match recorder {
			Some(recorder) => {
				let trie_state = state.as_trie_backend()
					.ok_or_else(||
						Box::new(state_machine::ExecutionError::UnableToGenerateProof)
							as Box<dyn state_machine::Error>
					)?;

				let backend = state_machine::ProvingBackend::new_with_recorder(
					trie_state,
					recorder.clone()
				);

				StateMachine::new(
					&backend,
					self.backend.changes_trie_storage(),
					side_effects_handler,
					&mut *changes.borrow_mut(),
					&self.executor,
					method,
					call_data,
					keystore,
				)
				.execute_using_consensus_failure_handler(
					execution_manager,
					false,
					native_call,
				)
				.map(|(result, _, _)| result)
				.map_err(Into::into)
			}
			None => StateMachine::new(
				&state,
				self.backend.changes_trie_storage(),
				side_effects_handler,
				&mut *changes.borrow_mut(),
				&self.executor,
				method,
				call_data,
				keystore,
			)
			.execute_using_consensus_failure_handler(
				execution_manager,
				false,
				native_call,
			)
			.map(|(result, _, _)| result)
		}?;
		self.backend.destroy_state(state)?;
		Ok(result)
	}

	fn runtime_version(&self, id: &BlockId<Block>) -> error::Result<RuntimeVersion> {
		let mut overlay = OverlayedChanges::default();
		let state = self.backend.state_at(*id)?;

		let mut ext = Ext::new(
			&mut overlay,
			&state,
			self.backend.changes_trie_storage(),
			None,
		);
		let version = self.executor.runtime_version(&mut ext);
		self.backend.destroy_state(state)?;
		version.ok_or(error::Error::VersionInvalid.into())
	}

	fn call_at_state<
		S: state_machine::Backend<Blake2Hasher>,
		F: FnOnce(
			Result<NativeOrEncoded<R>, Self::Error>,
			Result<NativeOrEncoded<R>, Self::Error>,
		) -> Result<NativeOrEncoded<R>, Self::Error>,
		R: Encode + Decode + PartialEq,
		NC: FnOnce() -> result::Result<R, String> + UnwindSafe,
	>(&self,
		state: &S,
		changes: &mut OverlayedChanges,
		method: &str,
		call_data: &[u8],
		manager: ExecutionManager<F>,
		native_call: Option<NC>,
		side_effects_handler: Option<OffchainExt>,
	) -> error::Result<(
		NativeOrEncoded<R>,
		(S::Transaction, <Blake2Hasher as Hasher>::Out),
		Option<ChangesTrieTransaction<Blake2Hasher, NumberFor<Block>>>,
	)> {
		StateMachine::new(
			state,
			self.backend.changes_trie_storage(),
			side_effects_handler,
			changes,
			&self.executor,
			method,
			call_data,
			self.keystore.clone().map(KeystoreExt),
		).execute_using_consensus_failure_handler(
			manager,
			true,
			native_call,
		)
		.map(|(result, storage_tx, changes_tx)| (
			result,
			storage_tx.expect("storage_tx is always computed when compute_tx is true; qed"),
			changes_tx,
		))
		.map_err(Into::into)
	}

	fn prove_at_trie_state<S: state_machine::TrieBackendStorage<Blake2Hasher>>(
		&self,
		trie_state: &state_machine::TrieBackend<S, Blake2Hasher>,
		overlay: &mut OverlayedChanges,
		method: &str,
		call_data: &[u8]
	) -> Result<(Vec<u8>, StorageProof), error::Error> {
		state_machine::prove_execution_on_trie_backend(
			trie_state,
			overlay,
			&self.executor,
			method,
			call_data,
			self.keystore.clone().map(KeystoreExt),
		)
		.map_err(Into::into)
	}

	fn native_runtime_version(&self) -> Option<&NativeVersion> {
		Some(self.executor.native_version())
	}
}
