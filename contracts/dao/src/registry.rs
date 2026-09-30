//! Name registry. Maps human-readable names to addresses (and back), so
//! members and the DAO itself can be referenced by name on-chain.

use soroban_sdk::{symbol_short, Address, Env, String};

use crate::error::Error;
use crate::storage::{self, DataKey};
use crate::types::{NAME_MAX_LEN, NAME_MIN_LEN};
use crate::util;

pub fn register_name(env: &Env, owner: Address, name: String) -> Result<(), Error> {
    util::require_initialized(env)?;
    util::require_not_paused(env)?;
    util_owner_auth(&owner);

    // Validate length bounds.
    let len = name.len();
    if !(NAME_MIN_LEN..=NAME_MAX_LEN).contains(&len) {
        return Err(Error::InvalidName);
    }

    // Validate charset: ASCII lowercase alphanumeric, '-', '_'.
    // No leading or trailing '-' or '_'.
    let mut buf = [0u8; 64];
    let len_usize = len as usize;
    name.copy_into_slice(&mut buf[..len_usize]);
    let first = buf[0];
    let last = buf[len_usize - 1];
    if first == b'-' || first == b'_' || last == b'-' || last == b'_' {
        return Err(Error::InvalidName);
    }
    for &b in buf.iter().take(len_usize) {
        let valid = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_';
        if !valid {
            return Err(Error::InvalidName);
        }
    }

    if let Some(existing) = storage::get_name_owner(env, &name) {
        if existing != owner {
            return Err(Error::NameTaken);
        }
    }

    // Free any name this owner held previously so lookups stay 1:1.
    if let Some(old) = storage::get_name_of(env, &owner) {
        if old != name {
            env.storage()
                .persistent()
                .remove(&DataKey::Name(old.clone()));
            // Announce the release so the set of free names is
            // reconstructable from the event log alone (issue #124).
            env.events()
                .publish((symbol_short!("name_rel"),), (old, owner.clone()));
        }
    }

    storage::set_name(env, &name, &owner);
    env.events()
        .publish((symbol_short!("name_reg"),), (name, owner));
    Ok(())
}

pub fn resolve_name(env: &Env, name: String) -> Option<Address> {
    storage::get_name_owner(env, &name)
}

pub fn name_of(env: &Env, owner: Address) -> Option<String> {
    storage::get_name_of(env, &owner)
}

fn util_owner_auth(owner: &Address) {
    owner.require_auth();
}
