use super::*;

use bincode::error::{DecodeError, EncodeError};
use std::cell::Cell;

static VERSION_STR: &str = "2023-03-01";

thread_local! {
    static ACTION_BASE: Cell<(*mut u8, *mut u8)> = Cell::new((ptr::null_mut(), ptr::null_mut()));
    static IP_BASE: Cell<*mut u8> = Cell::new(ptr::null_mut());
    static CHANCE_BASE: Cell<*mut u8> = Cell::new(ptr::null_mut());
}

impl Encode for PostFlopGame {
    fn encode<E: bincode::enc::Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        // store base pointers
        ACTION_BASE.with(|c| {
            if self.state >= State::MemoryAllocated {
                c.set((self.storage1.lock().as_mut_ptr(), ptr::null_mut()));
            } else {
                c.set((ptr::null_mut(), ptr::null_mut()));
            }
        });

        IP_BASE.with(|c| {
            if self.state >= State::MemoryAllocated {
                c.set(self.storage_ip.lock().as_mut_ptr());
            } else {
                c.set(ptr::null_mut());
            }
        });

        CHANCE_BASE.with(|c| {
            if self.state >= State::MemoryAllocated {
                c.set(self.storage_chance.lock().as_mut_ptr());
            } else {
                c.set(ptr::null_mut());
            }
        });

        // version
        VERSION_STR.to_string().encode(encoder)?;

        // action tree
        self.tree_config.encode(encoder)?;
        self.added_lines.encode(encoder)?;
        self.removed_lines.encode(encoder)?;

        // contents
        self.state.encode(encoder)?;
        self.card_config.encode(encoder)?;
        self.num_combinations.encode(encoder)?;
        self.is_compression_enabled.encode(encoder)?;
        self.num_storage_actions.encode(encoder)?;
        self.num_storage_chances.encode(encoder)?;
        self.misc_memory_usage.encode(encoder)?;
        self.storage1.encode(encoder)?;
        self.storage2.encode(encoder)?;
        self.storage_ip.encode(encoder)?;
        self.storage_chance.encode(encoder)?;
        self.locking_strategy.encode(encoder)?;
        self.history.encode(encoder)?;
        self.is_normalized_weight_cached.encode(encoder)?;

        // game tree
        self.node_arena.encode(encoder)?;

        Ok(())
    }
}

impl Decode for PostFlopGame {
    fn decode<D: bincode::de::Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        // version check
        let version = String::decode(decoder)?;
        if version != VERSION_STR {
            return Err(DecodeError::OtherString(format!(
                "Version mismatch: expected '{VERSION_STR}', but got '{version}'"
            )));
        }

        let tree_config = TreeConfig::decode(decoder)?;
        let added_lines = Vec::<Vec<Action>>::decode(decoder)?;
        let removed_lines = Vec::<Vec<Action>>::decode(decoder)?;

        let mut action_tree = ActionTree::new(tree_config).unwrap();
        for line in &added_lines {
            action_tree.add_line(line).unwrap();
        }
        for line in &removed_lines {
            action_tree.remove_line(line).unwrap();
        }

        let (tree_config, _, _, action_root) = action_tree.eject();

        // game instance
        let mut game = Self {
            state: Decode::decode(decoder)?,
            card_config: Decode::decode(decoder)?,
            tree_config,
            added_lines,
            removed_lines,
            action_root,
            num_combinations: Decode::decode(decoder)?,
            is_compression_enabled: Decode::decode(decoder)?,
            num_storage_actions: Decode::decode(decoder)?,
            num_storage_chances: Decode::decode(decoder)?,
            misc_memory_usage: Decode::decode(decoder)?,
            storage1: Decode::decode(decoder)?,
            storage2: Decode::decode(decoder)?,
            storage_ip: Decode::decode(decoder)?,
            storage_chance: Decode::decode(decoder)?,
            locking_strategy: Decode::decode(decoder)?,
            ..Default::default()
        };

        let history = Vec::<usize>::decode(decoder)?;
        let is_normalized_weight_cached = bool::decode(decoder)?;

        // store base pointers
        ACTION_BASE.with(|c| {
            if game.state >= State::MemoryAllocated {
                let base1 = game.storage1.lock().as_mut_ptr();
                let base2 = game.storage2.lock().as_mut_ptr();
                c.set((base1, base2));
            } else {
                c.set((ptr::null_mut(), ptr::null_mut()));
            }
        });

        IP_BASE.with(|c| {
            if game.state >= State::MemoryAllocated {
                c.set(game.storage_ip.lock().as_mut_ptr());
            } else {
                c.set(ptr::null_mut());
            }
        });

        CHANCE_BASE.with(|c| {
            if game.state >= State::MemoryAllocated {
                c.set(game.storage_chance.lock().as_mut_ptr());
            } else {
                c.set(ptr::null_mut());
            }
        });

        // game tree
        game.node_arena = Decode::decode(decoder)?;

        // initialization
        if game.state >= State::TreeBuilt {
            game.init_hands();
            game.init_card_fields();
            game.init_interpreter();

            game.apply_history(&history);
            if is_normalized_weight_cached {
                game.cache_normalized_weights();
            }
        }

        Ok(game)
    }
}

impl Encode for PostFlopNode {
    fn encode<E: bincode::enc::Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        // contents
        self.prev_action.encode(encoder)?;
        self.player.encode(encoder)?;
        self.turn.encode(encoder)?;
        self.river.encode(encoder)?;
        self.is_locked.encode(encoder)?;
        self.amount.encode(encoder)?;
        self.children_offset.encode(encoder)?;
        self.num_children.encode(encoder)?;
        self.num_elements_ip.encode(encoder)?;
        self.num_elements.encode(encoder)?;
        self.scale1.encode(encoder)?;
        self.scale2.encode(encoder)?;
        self.scale3.encode(encoder)?;

        // pointer offset
        if !self.storage1.is_null() {
            if self.is_terminal() {
                // do nothing
            } else if self.is_chance() {
                let offset = CHANCE_BASE.with(|c| unsafe { self.storage1.offset_from(c.get()) });
                offset.encode(encoder)?;
            } else {
                let offset = ACTION_BASE.with(|c| unsafe { self.storage1.offset_from(c.get().0) });
                let offset_ip = IP_BASE.with(|c| unsafe { self.storage3.offset_from(c.get()) });
                offset.encode(encoder)?;
                offset_ip.encode(encoder)?;
            }
        }

        Ok(())
    }
}

impl Decode for PostFlopNode {
    fn decode<D: bincode::de::Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        // node instance
        let mut node = Self {
            prev_action: Decode::decode(decoder)?,
            player: Decode::decode(decoder)?,
            turn: Decode::decode(decoder)?,
            river: Decode::decode(decoder)?,
            is_locked: Decode::decode(decoder)?,
            amount: Decode::decode(decoder)?,
            children_offset: Decode::decode(decoder)?,
            num_children: Decode::decode(decoder)?,
            num_elements_ip: Decode::decode(decoder)?,
            num_elements: Decode::decode(decoder)?,
            scale1: Decode::decode(decoder)?,
            scale2: Decode::decode(decoder)?,
            scale3: Decode::decode(decoder)?,
            ..Default::default()
        };

        // pointers
        if node.is_terminal() {
            // do nothing
        } else if node.is_chance() {
            let base = CHANCE_BASE.with(|c| c.get());
            if !base.is_null() {
                node.storage1 = unsafe { base.offset(isize::decode(decoder)?) };
            }
        } else {
            let base3 = IP_BASE.with(|c| c.get());
            if !base3.is_null() {
                let (base1, base2) = ACTION_BASE.with(|c| c.get());
                let offset = isize::decode(decoder)?;
                let offset_ip = isize::decode(decoder)?;
                node.storage1 = unsafe { base1.offset(offset) };
                node.storage2 = unsafe { base2.offset(offset) };
                node.storage3 = unsafe { base3.offset(offset_ip) };
            }
        }

        Ok(node)
    }
}