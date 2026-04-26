/**
 * Quick-start example for @novai/sdk.
 *
 * Walks through the four basic SDK operations:
 *   1. Connect to a node
 *   2. Fund an account from the faucet, check balance
 *   3. Transfer tokens to a second account
 *   4. Register an AI entity and verify it landed on chain
 *
 * Requires a local NOVAI devnet on http://localhost:3030.
 * See docs/tutorials/FIRST_AI_ENTITY.md for devnet setup.
 *
 * Run with `npm start` after `npm install`.
 */

import {
  AutonomyMode,
  bytesToHex,
  computeEntityId,
  generateKeypair,
  NovaiClient,
  registerAiEntityWithKey,
  transfer,
} from "@novai/sdk";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface LatestBlock {
  height: number;
  block_hash: string;
  state_root: string;
  tx_count: number;
}

async function main(): Promise<void> {
  // ---------------------------------------------------------------------
  // 1. Connect
  // ---------------------------------------------------------------------
  const client = new NovaiClient("http://localhost:3030");

  const latest = (await client.call(
    "novai_getLatestBlock",
    {},
  )) as LatestBlock | null;

  if (!latest) {
    throw new Error(
      "Chain has not committed any blocks yet. Is the devnet running? " +
        "See docs/tutorials/FIRST_AI_ENTITY.md for setup.",
    );
  }
  console.log(`Connected. Chain at height ${latest.height}.\n`);

  // ---------------------------------------------------------------------
  // 2. Generate two keypairs (sender + recipient)
  // ---------------------------------------------------------------------
  // generateKeypair() returns { seed, publicKey, address } — all Uint8Arrays.
  // The address is blake3("NOVAI_ADDRESS_V1" || publicKey).
  const sender = generateKeypair();
  const recipient = generateKeypair();
  console.log(`Sender    address: ${bytesToHex(sender.address)}`);
  console.log(`Recipient address: ${bytesToHex(recipient.address)}\n`);

  // ---------------------------------------------------------------------
  // 3. Fund the sender via the dev-mode faucet
  // ---------------------------------------------------------------------
  // Requires the node was launched with --dev-keys + --allow-insecure-dev-keys
  // (scripts/devnet.sh provides this). Dispenses 10,000,000 tokens per call,
  // 1-hour cooldown per address, 10s global cooldown.
  const faucetResult = await client.faucet(bytesToHex(sender.address));
  console.log(
    `Faucet dispensed ${faucetResult.amount} tokens (tx ${faucetResult.txid.slice(0, 16)}…).`,
  );
  await sleep(1500); // wait for inclusion

  const senderInitial = await client.getBalance(bytesToHex(sender.address));
  console.log(
    `Sender balance: ${senderInitial.balance}, nonce: ${senderInitial.nonce}\n`,
  );

  // ---------------------------------------------------------------------
  // 4. Send a transfer
  // ---------------------------------------------------------------------
  // transfer() builds a fully-signed TxV1. amount and fee are u64 (bigint).
  const transferAmount = 100_000n;
  const transferFee = 1_000n;
  const transferTx = transfer(
    sender,
    senderInitial.nonce,
    transferFee,
    recipient.address,
    transferAmount,
  );
  const transferTxid = await client.submitTx(transferTx);
  console.log(
    `Transfer ${transferAmount} tokens → recipient submitted (tx ${transferTxid.slice(0, 16)}…).`,
  );
  await sleep(1500);

  const senderAfterTransfer = await client.getBalance(
    bytesToHex(sender.address),
  );
  const recipientAfterTransfer = await client.getBalance(
    bytesToHex(recipient.address),
  );
  console.log(
    `Sender    balance: ${senderAfterTransfer.balance} (was ${senderInitial.balance})`,
  );
  console.log(`Recipient balance: ${recipientAfterTransfer.balance}\n`);

  // ---------------------------------------------------------------------
  // 5. Register an AI entity
  // ---------------------------------------------------------------------
  // registerAiEntityWithKey() registers a new on-chain entity with its own
  // signing key (separate from the creator's). The entity gets a canonical
  // id derived from (code_hash, creator_address). The fee must meet
  // MIN_FEE_REGISTER_AI_ENTITY_WITH_KEY (5,000).
  const entityKey = generateKeypair();
  const codeHash = new Uint8Array(32).fill(0x01); // opaque placeholder
  const initialBalance = 50_000n;
  const registerFee = 5_000n;

  const regTx = registerAiEntityWithKey(
    sender,
    senderAfterTransfer.nonce,
    registerFee,
    codeHash,
    entityKey.publicKey,
    AutonomyMode.Gated,
    {
      readPublicChain: true,
      readMemoryObjects: true,
      emitProposals: true,
    },
    initialBalance,
  );
  const regTxid = await client.submitTx(regTx);
  console.log(`Entity registration submitted (tx ${regTxid.slice(0, 16)}…).`);
  await sleep(1500);

  // ---------------------------------------------------------------------
  // 6. Verify the entity landed on chain
  // ---------------------------------------------------------------------
  // computeEntityId() mirrors the chain's deterministic derivation:
  //   entity_id = blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)
  const entityId = computeEntityId(codeHash, sender.address);
  const entityIdHex = bytesToHex(entityId);

  const entity = await client.getAiEntity(entityIdHex);
  if (!entity) {
    throw new Error(`Entity ${entityIdHex} not found after register`);
  }

  console.log(`\nEntity ${entityIdHex.slice(0, 16)}… on chain:`);
  console.log(`  creator:        ${entity.creator}`);
  console.log(`  pubkey:         ${entity.pubkey}`);
  console.log(`  balance:        ${entity.economic_balance}`);
  console.log(`  autonomy_mode:  ${entity.autonomy_mode} (Gated)`);
  console.log(
    `  capabilities:   0x${entity.capabilities.toString(16).padStart(2, "0")}`,
  );
  console.log(`  registered_at:  block ${entity.registered_at}`);
  console.log(`  is_active:      ${entity.is_active}`);
}

main().catch((err: unknown) => {
  const msg = err instanceof Error ? err.message : String(err);
  console.error("\nERROR:", msg);
  process.exit(1);
});
