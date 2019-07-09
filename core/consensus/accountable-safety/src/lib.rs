// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Common utilities for accountable safety in Substrate.

#![forbid(missing_docs, unsafe_code)]

use client;
use transaction_pool::txpool::{self, PoolApi};
use parity_codec::{Encode, Decode};
use runtime_primitives::traits::{Block as BlockT, ProvideRuntimeApi};
use runtime_primitives::generic::BlockId;
use log::info;
use client::blockchain::HeaderBackend;
use client::transaction_builder::api::TransactionBuilder;

/// Trait to submit report calls to the transaction pool.
pub trait SubmitReport<C, Block> {
	/// Submit report call to the transaction pool.
	fn submit_report_call(&self, client: &C, extrinsic: &[u8]);
}

impl<C, Block, T: PoolApi + Send + Sync + 'static> SubmitReport<C, Block> for T 
where 
	Block: BlockT + 'static,
	<T as PoolApi>::Api: txpool::ChainApi<Block=Block> + 'static,
	<Block as BlockT>::Extrinsic: Decode,
	C: HeaderBackend<Block> + ProvideRuntimeApi,
	C::Api: TransactionBuilder<Block>,
{
	fn submit_report_call(&self, client: &C, mut extrinsic: &[u8]) {
		info!(target: "accountable-safety", "Submitting report call to tx pool");
		if let Some(uxt) = Decode::decode(&mut extrinsic) {
			let block_id = BlockId::<Block>::number(client.info().best_number);
			if let Err(e) = self.submit_one(&block_id, uxt) {
				info!(target: "accountable-safety", "Error importing misbehavior report: {:?}", e);
			}
		} else {
			info!(target: "accountable-safety", "Error decoding report call");
		}
	}
}
