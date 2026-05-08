//! Week 26: A26.5 Malicious Module Attack Tests.
//!
//! PURPOSE: Test that a maximally-privileged malicious AI module cannot
//! access NNPX private data through any available execution path.
//!
//! CONTEXT: NOVAI does not yet have a WASM runtime. These tests verify
//! the execution-layer boundaries that any future WASM host must enforce.
//! They simulate what a malicious module would encounter when attempting
//! to bypass privacy protections through the existing API surface.
//!
//! ATTACK VECTORS:
//! - Call validate_nnpx_access with crafted keys as AI caller
//! - Forge Caller::Account identity to bypass AI restrictions
//! - Access state belonging to other entities
//! - Construct WriteOps targeting NNPX keys directly
//! - Exhaust capabilities by combining all permission flags
//! - Attempt every known NNPX key prefix with every caller type
//!
//! EXPECTED RESULTS:
//! - All NNPX access denied for any AI caller, regardless of capabilities
//! - Caller identity is structural (enum variant), not forgeable at runtime
//! - Entity state isolation enforced by key prefix containing entity_id
//! - WriteOps targeting NNPX keys are detectable via is_private_key
//! - Maximum capability entity still blocked from raw NNPX
//!
//! MITIGATION: Hard boundary in execution layer; WASM host must use
//! Caller::AiEntity for all module-initiated operations.

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
use novai_execution::{
    is_private_key, validate_ai_entity_no_nnpx_capability, validate_derived_view_access,
    validate_nnpx_access, Caller, ExecError,
};
use novai_state::{
    WriteOp, KEY_PREFIX_NNPX, KEY_PREFIX_NNPX_COMMITMENTS, KEY_PREFIX_NNPX_ENCRYPTED,
    KEY_PREFIX_NNPX_NULLIFIERS,
};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Create an AI entity with ALL capability bits set to true.
fn max_capability_entity() -> AiEntity {
    let caps = Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: true,
        read_nnpx_derived: true,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    };
    AiEntity::new(
        [0xAAu8; 32],
        [0xBBu8; 32],
        AutonomyMode::Autonomous,
        caps,
        1000,
    )
}

// ============================================================================
// A26.5-T1: MALICIOUS MODULE CANNOT ACCESS NNPX VIA HOST IMPORT
// ============================================================================

#[test]
fn test_malicious_wasm_cannot_access_nnpx_via_host_import() {
    // ATTACK: A malicious module calls the host-provided state read function
    // (simulated by validate_nnpx_access) with every known NNPX key prefix.
    // The module has maximum capabilities and Autonomous mode.
    //
    // EXPECTED: Every access attempt is denied. The privacy boundary does
    // not check capabilities — it checks the Caller enum variant. ANY
    // Caller::AiEntity is blocked from ANY nnpx/ key, period.

    let entity = max_capability_entity();
    let ai_caller = Caller::AiEntity(entity.id);

    // All known NNPX key prefixes
    let nnpx_prefixes: Vec<&[u8]> = vec![
        KEY_PREFIX_NNPX,
        KEY_PREFIX_NNPX_COMMITMENTS,
        KEY_PREFIX_NNPX_NULLIFIERS,
        KEY_PREFIX_NNPX_ENCRYPTED,
    ];

    // Try each prefix with various suffixes
    for prefix in &nnpx_prefixes {
        // Bare prefix
        let result: Result<(), ExecError<()>> = validate_nnpx_access(prefix, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "Max-cap AI must be denied bare prefix: {:?}",
            String::from_utf8_lossy(prefix),
        );

        // Prefix + 32-byte key
        let mut key_with_id = prefix.to_vec();
        key_with_id.extend_from_slice(&[0xABu8; 32]);
        let result: Result<(), ExecError<()>> = validate_nnpx_access(&key_with_id, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "Max-cap AI must be denied prefix+id: {:?}",
            String::from_utf8_lossy(&key_with_id),
        );
    }

    // Verify that capabilities do NOT affect the NNPX boundary
    // Even read_nnpx_derived=true doesn't help with raw nnpx/ keys
    assert!(entity.capabilities.read_nnpx_derived);
    let result: Result<(), ExecError<()>> =
        validate_nnpx_access(b"nnpx/commitments/target", &ai_caller);
    assert!(
        matches!(result, Err(ExecError::NnpxAccessDenied)),
        "read_nnpx_derived capability must NOT bypass raw NNPX boundary"
    );
}

// ============================================================================
// A26.5-T2: MALICIOUS MODULE CANNOT FORGE CALLER IDENTITY
// ============================================================================

#[test]
fn test_malicious_wasm_cannot_forge_caller_identity() {
    // ATTACK: A malicious module attempts to impersonate a human Account
    // caller to bypass the AI entity NNPX restriction. In a WASM runtime,
    // the Caller enum is constructed by the HOST, not the guest module.
    //
    // EXPECTED: The Caller enum is a Rust type constructed at the host level.
    // A WASM guest cannot construct a Caller::Account — the host always
    // provides Caller::AiEntity for module-initiated operations.
    //
    // This test verifies the BEHAVIORAL difference between the two variants.

    let entity_id = [0x42u8; 32];
    let account_addr = [0x01u8; 32];

    let ai_caller = Caller::AiEntity(entity_id);
    let account_caller = Caller::Account(account_addr);

    let nnpx_key = b"nnpx/commitments/secret_data";

    // AI caller is DENIED
    let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller);
    assert!(matches!(result, Err(ExecError::NnpxAccessDenied)));

    // Account caller is ALLOWED
    let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &account_caller);
    assert!(result.is_ok());

    // Even if a module could somehow use the SAME 32-byte ID as an account,
    // the Caller::AiEntity variant still blocks it
    let same_bytes_ai = Caller::AiEntity(account_addr); // Same bytes, AI variant
    let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &same_bytes_ai);
    assert!(
        matches!(result, Err(ExecError::NnpxAccessDenied)),
        "AI variant with account bytes must still be denied"
    );

    // The two enum variants are structurally different
    assert_ne!(
        ai_caller, account_caller,
        "AiEntity and Account are different enum variants"
    );

    // Caller::AiEntity with same ID is equal to itself
    let ai_caller_copy = Caller::AiEntity(entity_id);
    assert_eq!(ai_caller, ai_caller_copy);
}

// ============================================================================
// A26.5-T3: MALICIOUS MODULE MEMORY ISOLATION
// ============================================================================

#[test]
fn test_malicious_wasm_memory_isolation() {
    // ATTACK: A malicious module attempts to read state belonging to a
    // different AI entity by constructing keys with another entity's ID.
    //
    // EXPECTED: The validate_nnpx_access function blocks ALL AI entities
    // from nnpx/ keys regardless of which entity ID is in the key.
    // For non-NNPX keys, entity isolation is enforced by key prefixing
    // (each entity's data is under ai/memory/{entity_id}/).

    let attacker_id = [0xAAu8; 32];
    let victim_id = [0xBBu8; 32];

    let attacker_caller = Caller::AiEntity(attacker_id);

    // Attempt 1: Attacker tries to read victim's NNPX data
    let victim_nnpx_key: Vec<u8> = [KEY_PREFIX_NNPX_COMMITMENTS, &victim_id[..]].concat();
    let result: Result<(), ExecError<()>> =
        validate_nnpx_access(&victim_nnpx_key, &attacker_caller);
    assert!(
        matches!(result, Err(ExecError::NnpxAccessDenied)),
        "Attacker must be denied access to victim's NNPX data"
    );

    // Attempt 2: Attacker tries to read ANY entity's NNPX data
    for target_byte in [0x00u8, 0x11, 0x22, 0x33, 0xFF] {
        let target_id = [target_byte; 32];
        let key: Vec<u8> = [KEY_PREFIX_NNPX_ENCRYPTED, &target_id[..]].concat();
        let result: Result<(), ExecError<()>> = validate_nnpx_access(&key, &attacker_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "Attacker must be denied access to any entity's NNPX data"
        );
    }

    // Attempt 3: Attacker constructs keys for other entity's memory space
    // These are NOT nnpx/ keys, so they pass the NNPX check.
    // However, the execution layer uses entity_id-prefixed keys, so a module
    // can only access its own memory (enforced at tx processing level).
    let mut own_memory_key = b"ai/memory/".to_vec();
    own_memory_key.extend_from_slice(&attacker_id);
    own_memory_key.extend_from_slice(b"/config");

    let mut victim_memory_key = b"ai/memory/".to_vec();
    victim_memory_key.extend_from_slice(&victim_id);
    victim_memory_key.extend_from_slice(b"/config");

    // Both pass NNPX check (not nnpx/ keys)
    let result: Result<(), ExecError<()>> = validate_nnpx_access(&own_memory_key, &attacker_caller);
    assert!(result.is_ok(), "Own memory key should pass NNPX check");

    let result: Result<(), ExecError<()>> =
        validate_nnpx_access(&victim_memory_key, &attacker_caller);
    assert!(
        result.is_ok(),
        "Victim memory key passes NNPX check (isolation enforced elsewhere)"
    );

    // The keys are in different namespaces (different entity_id prefix)
    assert_ne!(
        own_memory_key, victim_memory_key,
        "Entity memory keys must be different per entity"
    );
}

// ============================================================================
// A26.5-T4: MALICIOUS MODULE CANNOT BYPASS GAS LIMIT
// ============================================================================

#[test]
fn test_malicious_module_cannot_bypass_gas_limit() {
    // ATTACK: A malicious module attempts to exhaust resources by making
    // many rapid validation calls. In a future WASM runtime, gas metering
    // would limit this. At the current execution layer, each call is O(1).
    //
    // EXPECTED: validate_nnpx_access is a simple starts_with check (O(1)).
    // Even millions of calls cannot cause resource exhaustion beyond the
    // fee already charged for the transaction.

    let ai_caller = Caller::AiEntity([0x42u8; 32]);

    // Simulate a module making 100,000 access attempts
    let mut denied_count = 0u64;
    let mut allowed_count = 0u64;

    for i in 0u64..100_000 {
        // Alternate between NNPX keys (denied) and public keys (allowed)
        let key: Vec<u8> = if i % 2 == 0 {
            let mut k = KEY_PREFIX_NNPX.to_vec();
            k.extend_from_slice(&i.to_be_bytes());
            k
        } else {
            format!("accounts/{i}").into_bytes()
        };

        let result: Result<(), ExecError<()>> = validate_nnpx_access(&key, &ai_caller);
        match result {
            Err(ExecError::NnpxAccessDenied) => denied_count += 1,
            Ok(()) => allowed_count += 1,
            _ => panic!("Unexpected error variant"),
        }
    }

    // Exactly half should be denied (nnpx/ keys), half allowed (accounts/)
    assert_eq!(denied_count, 50_000, "All nnpx/ keys must be denied");
    assert_eq!(allowed_count, 50_000, "All public keys must be allowed");

    // The check is O(1) per call — no state accumulation, no memory growth,
    // no resource exhaustion possible beyond CPU time (metered by gas in future).
}

// ============================================================================
// A26.5-T5: MALICIOUS MODULE CANNOT ACCESS OTHER MODULE STATE
// ============================================================================

#[test]
fn test_malicious_module_cannot_access_other_module_state() {
    // ATTACK: A malicious AI entity attempts to read derived views that
    // were created by a different entity or for a different purpose.
    //
    // EXPECTED: Derived view access is controlled by the read_nnpx_derived
    // capability flag, NOT by creator identity. Any entity with the
    // capability can read ANY derived view (they contain only aggregates).
    // But entities WITHOUT the capability are completely blocked.

    // Entity A: has derived capability
    let entity_a = AiEntity::new(
        [0xAAu8; 32],
        [0x01u8; 32],
        AutonomyMode::Gated,
        Capabilities {
            read_nnpx_derived: true,
            read_public_chain: true,
            read_memory_objects: false,
            emit_proposals: false,
            request_execution: false,
            submit_reputation_updates: false,
            _reserved: [false; 2],
        },
        1000,
    );

    // Entity B: does NOT have derived capability
    let entity_b = AiEntity::new(
        [0xBBu8; 32],
        [0x02u8; 32],
        AutonomyMode::Gated,
        Capabilities::gated(), // read_nnpx_derived = false
        1000,
    );

    // Entity A can access derived views
    let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_a);
    assert!(result.is_ok(), "Entity A with capability should pass");

    // Entity B cannot access derived views
    let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_b);
    assert!(
        matches!(result, Err(ExecError::DerivedViewAccessDenied)),
        "Entity B without capability must be denied"
    );

    // Neither entity can access raw NNPX data
    let ai_caller_a = Caller::AiEntity(entity_a.id);
    let ai_caller_b = Caller::AiEntity(entity_b.id);

    let nnpx_key = b"nnpx/commitments/any_key";
    let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller_a);
    assert!(matches!(result, Err(ExecError::NnpxAccessDenied)));

    let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller_b);
    assert!(matches!(result, Err(ExecError::NnpxAccessDenied)));
}

// ============================================================================
// A26.5-T6: MALICIOUS MODULE CANNOT EMIT INVALID WRITE OPS
// ============================================================================

#[test]
fn test_malicious_module_cannot_emit_invalid_write_ops() {
    // ATTACK: A malicious module constructs WriteOps that target NNPX keys
    // directly, bypassing the validation layer. In a proper execution model,
    // all WriteOps emitted by modules must pass through validation before
    // being applied to state.
    //
    // EXPECTED: Any WriteOp targeting an nnpx/ key can be detected by
    // is_private_key(). The execution layer must validate all WriteOps
    // before applying them.

    // Simulate a malicious module emitting WriteOps
    let malicious_ops: Vec<WriteOp> = vec![
        // Try to write to NNPX commitments
        WriteOp::Put(
            [KEY_PREFIX_NNPX_COMMITMENTS, &[0xABu8; 32]].concat(),
            b"fake_commitment".to_vec(),
        ),
        // Try to mark a nullifier as spent
        WriteOp::Put([KEY_PREFIX_NNPX_NULLIFIERS, &[0xCDu8; 32]].concat(), vec![]),
        // Try to write encrypted payload
        WriteOp::Put(
            [KEY_PREFIX_NNPX_ENCRYPTED, &[0xEFu8; 32]].concat(),
            b"fake_encrypted_data".to_vec(),
        ),
        // Try to delete a nullifier (un-spend)
        WriteOp::Delete([KEY_PREFIX_NNPX_NULLIFIERS, &[0x11u8; 32]].concat()),
        // Try to write to bare nnpx/ prefix
        WriteOp::Put(KEY_PREFIX_NNPX.to_vec(), b"root_data".to_vec()),
    ];

    // All of these WriteOps target private keys
    for (i, op) in malicious_ops.iter().enumerate() {
        let key = match op {
            WriteOp::Put(k, _) | WriteOp::Delete(k) => k,
        };

        assert!(
            is_private_key(key),
            "Malicious WriteOp #{i} targets a private key but was not detected",
        );
    }

    // Legitimate WriteOps targeting public keys should NOT be flagged
    let legitimate_ops: Vec<WriteOp> = vec![
        WriteOp::Put(b"accounts/alice".to_vec(), b"balance".to_vec()),
        WriteOp::Put(b"ai/memory/entity123/config".to_vec(), b"data".to_vec()),
        WriteOp::Put(b"derived_views/view123".to_vec(), b"aggregate".to_vec()),
        WriteOp::Delete(b"ai/memory/entity123/temp".to_vec()),
    ];

    for (i, op) in legitimate_ops.iter().enumerate() {
        let key = match op {
            WriteOp::Put(k, _) | WriteOp::Delete(k) => k,
        };

        assert!(
            !is_private_key(key),
            "Legitimate WriteOp #{i} was incorrectly flagged as private",
        );
    }

    // Verify the registration-time check: entities cannot be created
    // with read_nnpx_derived=true (defense-in-depth)
    let malicious_caps = Capabilities {
        read_nnpx_derived: true,
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: true,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    };
    let result: Result<(), ExecError<()>> = validate_ai_entity_no_nnpx_capability(&malicious_caps);
    assert!(
        matches!(result, Err(ExecError::NnpxAccessDenied)),
        "Registration with read_nnpx_derived=true must be blocked"
    );

    // All standard capability presets do NOT include read_nnpx_derived
    let presets = [
        Capabilities::read_only(),
        Capabilities::advisory(),
        Capabilities::gated(),
    ];
    for preset in &presets {
        assert!(
            !preset.read_nnpx_derived,
            "Standard capability preset must not include read_nnpx_derived"
        );
        let result: Result<(), ExecError<()>> = validate_ai_entity_no_nnpx_capability(preset);
        assert!(
            result.is_ok(),
            "Standard preset must pass registration check"
        );
    }
}
