use core::panic;

use massa_consensus_exports::{
    block_status::{BlockStatus, DiscardReason, HeaderOrBlock},
    error::ConsensusError,
};
use massa_logging::massa_trace;
use massa_models::{
    active_block::ActiveBlock,
    block_id::BlockId,
    prehash::{PreHashMap, PreHashSet},
    slot::Slot,
};
use tracing::debug;

use super::ConsensusState;

impl ConsensusState {
    /// prune active blocks and return final blocks, return discarded final blocks
    fn prune_active(&mut self) -> Result<PreHashMap<BlockId, ActiveBlock>, ConsensusError> {
        // list required active blocks
        let mut retain_active: PreHashSet<BlockId> = self.list_required_active_blocks(None)?;

        // retain extra history according to the config
        // this is useful to avoid desync on temporary connection loss
        for a_block in self.blocks_state.active_blocks().clone().iter() {
            if let Some(BlockStatus::Active {
                a_block: active_block,
                storage_or_block,
            }) = self.blocks_state.get_mut(a_block)
            {
                let (_b_id, latest_final_period) =
                    self.latest_final_blocks_periods[active_block.slot.thread as usize];

                if active_block.slot.period
                    >= latest_final_period
                        .saturating_sub(self.config.force_keep_final_periods_without_ops)
                {
                    retain_active.insert(*a_block);
                    if active_block.slot.period
                        < latest_final_period.saturating_sub(self.config.force_keep_final_periods)
                        && !self.active_index_without_ops.contains(a_block)
                    {
                        storage_or_block.strip_to_block(a_block);
                        self.active_index_without_ops.insert(*a_block);
                        // reset the list of descendants
                        active_block.descendants = Default::default();
                    }
                } else {
                    self.active_index_without_ops.remove(a_block);
                }
            }
        }

        // remove unused final active blocks
        let mut discarded_finals: PreHashMap<BlockId, ActiveBlock> = PreHashMap::default();
        let to_remove: Vec<BlockId> = self
            .blocks_state
            .active_blocks()
            .difference(&retain_active)
            .copied()
            .collect();
        for discard_active_h in to_remove {
            let sequence_number = self.blocks_state.sequence_counter();
            self.blocks_state.transition_map(&discard_active_h, |block_status, block_statuses| {
                if let Some(
                    BlockStatus::Active {
                        a_block: discarded_active,
                        ..
                    }
                ) = block_status {
                    // remove from parent's children
                    for (parent_h, _parent_period) in discarded_active.parents.iter() {
                        if let Some(BlockStatus::Active {
                            a_block: parent_active_block,
                            ..
                        }) = block_statuses.get_mut(parent_h)
                        {
                            parent_active_block.children[discarded_active.slot.thread as usize]
                                .remove(&discard_active_h);
                        }
                    }

                    massa_trace!("consensus.block_graph.prune_active", {"hash": discard_active_h, "reason": DiscardReason::Final});
                    let block_slot = discarded_active.slot;
                    let block_creator = discarded_active.creator_address;
                    let block_parents = discarded_active.parents.iter().map(|(p, _)| *p).collect();
                    discarded_finals.insert(discard_active_h, *discarded_active);

                    // mark as final
                    Some(BlockStatus::Discarded {
                        slot: block_slot,
                        creator: block_creator,
                        parents: block_parents,
                        reason: DiscardReason::Final,
                        sequence_number,
                    })
                } else {
                    panic!("inconsistency inside block statuses pruning and removing unused final active blocks - {} is missing", discard_active_h);
                }
            });
        }
        Ok(discarded_finals)
    }

    // Keep only a certain (`config.max_future_processing_blocks`) number of blocks that have slots in the future
    // to avoid high memory consumption
    fn prune_slot_waiting(&mut self) {
        if self.blocks_state.waiting_for_slot_blocks().len()
            <= self.config.max_future_processing_blocks
        {
            return;
        }
        let mut slot_waiting: Vec<(Slot, BlockId)> = self
            .blocks_state
            .waiting_for_slot_blocks()
            .iter()
            .filter_map(|block_id| {
                if let Some(BlockStatus::WaitingForSlot(header_or_block)) =
                    self.blocks_state.get(block_id)
                {
                    return Some((header_or_block.get_slot(), *block_id));
                }
                None
            })
            .collect();
        slot_waiting.sort_unstable();
        let len_slot_waiting = slot_waiting.len();
        (self.config.max_future_processing_blocks..len_slot_waiting).for_each(|idx| {
            let (_slot, block_id) = &slot_waiting[idx];
            self.blocks_state.transition_map(block_id, |_, _| None);
        });
    }

    // Keep only a certain (`config.max_discarded_blocks`) number of blocks that are discarded
    // to avoid high memory consumption
    fn prune_discarded(&mut self) -> Result<(), ConsensusError> {
        if self.blocks_state.discarded_blocks().len() <= self.config.max_discarded_blocks {
            return Ok(());
        }
        let mut discard_hashes: Vec<(u64, BlockId)> = self
            .blocks_state
            .discarded_blocks()
            .iter()
            .filter_map(|block_id| {
                if let Some(BlockStatus::Discarded {
                    sequence_number, ..
                }) = self.blocks_state.get(block_id)
                {
                    return Some((*sequence_number, *block_id));
                }
                None
            })
            .collect();
        discard_hashes.sort_unstable();
        discard_hashes.truncate(
            self.blocks_state.discarded_blocks().len() - self.config.max_discarded_blocks,
        );
        for (_, block_id) in discard_hashes.iter() {
            self.blocks_state.transition_map(block_id, |_, _| None);
        }
        Ok(())
    }

    fn prune_waiting_for_dependencies(&mut self) -> Result<(), ConsensusError> {
        let mut to_discard: PreHashMap<BlockId, Option<DiscardReason>> = PreHashMap::default();
        let mut to_keep: PreHashMap<BlockId, (u64, Slot)> = PreHashMap::default();

        // list items that are older than the latest final blocks in their threads or have deps that are discarded
        {
            for block_id in self.blocks_state.waiting_for_dependencies_blocks().iter() {
                if let Some(BlockStatus::WaitingForDependencies {
                    header_or_block,
                    unsatisfied_dependencies,
                    sequence_number,
                }) = self.blocks_state.get(block_id)
                {
                    // has already discarded dependencies => discard (choose worst reason)
                    let mut discard_reason = None;
                    let mut discarded_dep_found = false;
                    for dep in unsatisfied_dependencies.iter() {
                        if let Some(BlockStatus::Discarded { reason, .. }) =
                            self.blocks_state.get(dep)
                        {
                            discarded_dep_found = true;
                            match reason {
                                DiscardReason::Invalid(reason) => {
                                    discard_reason = Some(DiscardReason::Invalid(format!("discarded because depend on block:{} that has discard reason:{}", block_id, reason)));
                                    break;
                                }
                                DiscardReason::Stale => discard_reason = Some(DiscardReason::Stale),
                                DiscardReason::Final => discard_reason = Some(DiscardReason::Stale),
                            }
                        }
                    }
                    if discarded_dep_found {
                        to_discard.insert(*block_id, discard_reason);
                        continue;
                    }

                    // is at least as old as the latest final block in its thread => discard as stale
                    let slot = header_or_block.get_slot();
                    if slot.period <= self.latest_final_blocks_periods[slot.thread as usize].1 {
                        to_discard.insert(*block_id, Some(DiscardReason::Stale));
                        continue;
                    }

                    // otherwise, mark as to_keep
                    to_keep.insert(*block_id, (*sequence_number, header_or_block.get_slot()));
                }
            }
        }

        // discard in chain and because of limited size
        while !to_keep.is_empty() {
            // mark entries as to_discard and remove them from to_keep
            for (hash, _old_order) in to_keep.clone().into_iter() {
                if let Some(BlockStatus::WaitingForDependencies {
                    unsatisfied_dependencies,
                    ..
                }) = self.blocks_state.get(&hash)
                {
                    // has dependencies that will be discarded => discard (choose worst reason)
                    let mut discard_reason = None;
                    let mut dep_to_discard_found = false;
                    for dep in unsatisfied_dependencies.iter() {
                        if let Some(reason) = to_discard.get(dep) {
                            dep_to_discard_found = true;
                            match reason {
                                Some(DiscardReason::Invalid(reason)) => {
                                    discard_reason = Some(DiscardReason::Invalid(format!("discarded because depend on block:{} that has discard reason:{}", hash, reason)));
                                    break;
                                }
                                Some(DiscardReason::Stale) => {
                                    discard_reason = Some(DiscardReason::Stale)
                                }
                                Some(DiscardReason::Final) => {
                                    discard_reason = Some(DiscardReason::Stale)
                                }
                                None => {} // leave as None
                            }
                        }
                    }
                    if dep_to_discard_found {
                        to_keep.remove(&hash);
                        to_discard.insert(hash, discard_reason);
                        continue;
                    }
                }
            }

            // remove worst excess element
            if to_keep.len() > self.config.max_dependency_blocks {
                let remove_elt = to_keep
                    .iter()
                    .filter_map(|(hash, _old_order)| {
                        if let Some(BlockStatus::WaitingForDependencies {
                            header_or_block,
                            sequence_number,
                            ..
                        }) = self.blocks_state.get(hash)
                        {
                            return Some((sequence_number, header_or_block.get_slot(), *hash));
                        }
                        None
                    })
                    .min();
                if let Some((_seq_num, _slot, hash)) = remove_elt {
                    to_keep.remove(&hash);
                    to_discard.insert(hash, None);
                    continue;
                }
            }

            // nothing happened: stop loop
            break;
        }

        // transition states to Discarded if there is a reason, otherwise just drop
        for (block_id, reason_opt) in to_discard.drain() {
            let sequence_number = self.blocks_state.sequence_counter();
            self.blocks_state.transition_map(&block_id, |block_status, _| {
                if let Some(BlockStatus::WaitingForDependencies {
                    header_or_block, ..
                }) = block_status {
                    let header = match header_or_block {
                        HeaderOrBlock::Header(h) => h,
                        HeaderOrBlock::Block { id: block_id, .. } => self
                            .storage
                            .read_blocks()
                            .get(&block_id)
                            .unwrap_or_else(|| panic!("block {} should be in storage", block_id))
                            .content
                            .header
                            .clone()
                    };
                    massa_trace!("consensus.block_graph.prune_waiting_for_dependencies", {"hash": block_id, "reason": reason_opt});
                    if let Some(reason) = reason_opt {
                        // add to stats if reason is Stale
                        if reason == DiscardReason::Stale {
                            self.new_stale_blocks.insert(
                                block_id,
                                (header.content_creator_address, header.content.slot),
                            );
                        }
                        // transition to Discarded only if there is a reason
                        Some(BlockStatus::Discarded {
                                slot: header.content.slot,
                                creator: header.content_creator_address,
                                parents: header.content.parents,
                                reason,
                                sequence_number,
                            },
                        )
                    } else {
                        None
                    }
                } else {
                    panic!("block {} should be in WaitingForDependencies state", block_id);
                }
            });
        }

        Ok(())
    }

    /// Clear the cache of blocks indexed by slot.
    /// Slot are not saved anymore, when the block in the same thread with a equal or greater period is finalized.
    fn prune_nonfinal_blocks_per_slot(&mut self) {
        self.nonfinal_active_blocks_per_slot
            .retain(|s, _| s.period > self.latest_final_blocks_periods[s.thread as usize].1);
    }

    /// Clear all the caches and blocks waiting to be processed to avoid too much memory usage.
    pub fn prune(&mut self) -> Result<(), ConsensusError> {
        let before = self.max_cliques.len();
        // Step 1: discard final blocks that are not useful to the graph anymore and return them
        self.prune_active()?;

        // Step 2: prune slot waiting blocks
        self.prune_slot_waiting();

        // Step 3: prune dependency waiting blocks
        self.prune_waiting_for_dependencies()?;

        // Step 4: prune discarded
        self.prune_discarded()?;

        // Step 5: prune nonfinal blocks per slot
        self.prune_nonfinal_blocks_per_slot();

        let after = self.max_cliques.len();
        if before != after {
            debug!(
                "clique number went from {} to {} after pruning",
                before, after
            );
        }

        Ok(())
    }
}
