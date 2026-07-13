//! Gate F4 (A0 amendment) RED tests: the execution-crate key families the
//! original classify table missed.
//!
//! Field finding: a0 inspect on real healthy-node dirs reported 13,502
//! unknown keys, all under ai/oracle_anchors/by_entity/. Root cause: the
//! diagnosis enumerated key constants from crates/state only, while the
//! execution crate defines its own families (oracle anchors, payments,
//! payment splits and conditions, SLAs, channels, VK registry, entity
//! upgrades, treasuries). Their writers all route through
//! apply_state_ops_with_smt (the anchor store: execution lib 9001-9043), so
//! they are SMT-committed state and must be admitted by A3 and included in
//! the rebuild. treasury/privacy is the one exception: defined but with zero
//! production usage, so its presence must fail closed.
//!
//! RED against the committed A0 (which classifies these keys as unknown);
//! flips green with unchanged bodies once classify.rs carries the families.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{build_fixture, run_a0, FixtureSpec};
use novai_state::Kv;
use novai_execution::{
    KEY_AI_TREASURY, KEY_MARKETPLACE_TREASURY, KEY_PREFIX_AI_CHANNELS_BY_PARTY_A,
    KEY_PREFIX_AI_CHANNELS_BY_PARTY_B, KEY_PREFIX_AI_ENTITY_UPGRADES_BY_ENTITY,
    KEY_PREFIX_AI_ENTITY_UPGRADES_SUMMARY, KEY_PREFIX_AI_ORACLE_ANCHORS_BY_ENTITY,
    KEY_PREFIX_AI_ORACLE_ANCHORS_BY_HASH, KEY_PREFIX_AI_ORACLE_ANCHORS_BY_TAG,
    KEY_PREFIX_AI_ORACLE_ANCHORS_SUMMARY, KEY_PREFIX_AI_PAYMENTS_BY_HASH,
    KEY_PREFIX_AI_PAYMENTS_BY_PAYEE, KEY_PREFIX_AI_PAYMENTS_BY_PAYER,
    KEY_PREFIX_AI_PAYMENT_CONDITIONS_BY_HASH, KEY_PREFIX_AI_PAYMENT_SPLITS_BY_HASH,
    KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY,
    KEY_PREFIX_AI_SLAS_ACTIVE_BETWEEN, KEY_PREFIX_AI_SLAS_BY_BUYER,
    KEY_PREFIX_AI_SLAS_BY_SELLER, KEY_PREFIX_AI_VK_REGISTRY_BY_ID, KEY_PRIVACY_TREASURY,
    KEY_SLASH_TREASURY,
};

/// prefix ++ filler bytes, the shape the classifier sees (it inspects
/// prefixes only; the SMT commits to raw key/value bytes).
fn fam_key(prefix: &[u8], filler: &[u8]) -> Vec<u8> {
    let mut k = prefix.to_vec();
    k.extend_from_slice(filler);
    k
}

/// One exemplar per newly proven SMT-committed family: 18 prefixed keys plus
/// the three written treasury singletons. by_entity, by_tag and the service
/// descriptor by_category index carry empty values, matching the production
/// scan-marker shape.
fn family_exemplars() -> Vec<(Vec<u8>, Vec<u8>)> {
    let id = [0x66; 32];
    vec![
        (fam_key(KEY_PREFIX_AI_ORACLE_ANCHORS_BY_HASH, &id), b"rec".to_vec()),
        (fam_key(KEY_PREFIX_AI_ORACLE_ANCHORS_BY_ENTITY, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_ORACLE_ANCHORS_BY_TAG, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_ORACLE_ANCHORS_SUMMARY, &id), b"sum".to_vec()),
        (fam_key(KEY_PREFIX_AI_PAYMENTS_BY_HASH, &id), b"pay".to_vec()),
        (fam_key(KEY_PREFIX_AI_PAYMENTS_BY_PAYER, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_PAYMENT_SPLITS_BY_HASH, &id), b"spl".to_vec()),
        (fam_key(KEY_PREFIX_AI_PAYMENT_CONDITIONS_BY_HASH, &id), b"cnd".to_vec()),
        (fam_key(KEY_PREFIX_AI_SLAS_ACTIVE_BETWEEN, &id), b"sla".to_vec()),
        (fam_key(KEY_PREFIX_AI_SLAS_BY_BUYER, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_SLAS_BY_SELLER, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_CHANNELS_BY_PARTY_A, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_CHANNELS_BY_PARTY_B, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_VK_REGISTRY_BY_ID, &id), b"vk".to_vec()),
        (fam_key(KEY_PREFIX_AI_ENTITY_UPGRADES_SUMMARY, &id), b"up".to_vec()),
        (fam_key(KEY_PREFIX_AI_ENTITY_UPGRADES_BY_ENTITY, &id), vec![]),
        (fam_key(KEY_PREFIX_AI_SERVICE_DESCRIPTORS_BY_CATEGORY, &id), vec![]),
        (KEY_AI_TREASURY.to_vec(), 7u128.to_be_bytes().to_vec()),
        (KEY_MARKETPLACE_TREASURY.to_vec(), 8u128.to_be_bytes().to_vec()),
        (KEY_SLASH_TREASURY.to_vec(), 9u128.to_be_bytes().to_vec()),
    ]
}

#[test]
fn all_execution_families_audit_clean_and_count_as_smt_committed() {
    let mut pre = a0_common::default_pre_state();
    pre.extend(family_exemplars());
    let fx = build_fixture(
        "fam_all",
        FixtureSpec {
            pre_state: pre,
            ..FixtureSpec::default()
        },
    );
    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(
        code, 0,
        "execution-family state must audit clean; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // 3 default accounts + 21 family exemplars + 1 step account = 25 leaves,
    // all counted in the SMT-committed class.
    assert!(
        stdout.contains("A3 PASS smt_committed=25"),
        "family keys must be counted as smt_committed; stdout:\n{stdout}"
    );
    assert_eq!(a0_common::parse_result_root(&stdout), hex::encode(fx.r1));
}

#[test]
fn oracle_by_entity_marker_alone_audits_clean() {
    // The exact shape reported in the field: by_entity scan markers with
    // empty values (execution lib 9008-9011).
    let mut pre = a0_common::default_pre_state();
    pre.push((
        fam_key(
            KEY_PREFIX_AI_ORACLE_ANCHORS_BY_ENTITY,
            &[[0x21; 32].as_slice(), &9u64.to_be_bytes(), &[0x22; 32]].concat(),
        ),
        vec![],
    ));
    let fx = build_fixture(
        "fam_oracle",
        FixtureSpec {
            pre_state: pre,
            ..FixtureSpec::default()
        },
    );
    let (code, stdout, stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(
        code, 0,
        "oracle by_entity marker must audit clean; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("A3 PASS smt_committed=5"),
        "stdout:\n{stdout}"
    );
    assert_eq!(a0_common::parse_result_root(&stdout), hex::encode(fx.r1));
}

#[test]
fn treasury_privacy_key_fails_a3_as_defined_but_unwritten() {
    let fx = build_fixture("fam_privacy", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(KEY_PRIVACY_TREASURY, &1u128.to_be_bytes())
            .expect("raw privacy treasury");
    }
    let (code, stdout, _stderr) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(
        code, 1,
        "treasury/privacy has no production writer and must fail; stdout:\n{stdout}"
    );
    assert!(stdout.contains("A3 FAIL"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("defined-but-unwritten"),
        "must be classified defined-but-unwritten, not unknown; stdout:\n{stdout}"
    );
    assert!(stdout.contains("treasury/privacy"), "stdout:\n{stdout}");
}
