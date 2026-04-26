/**
 * Typed JSON-RPC client for the NOVAI node.
 *
 * Talks to /rpc, which Vite's dev server proxies to http://localhost:3030.
 * In production, serve the built explorer behind a reverse proxy that
 * forwards /rpc to your node.
 */

const RPC_PATH = "/rpc";

let requestId = 1;

export class RpcError extends Error {
  code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
    this.name = "RpcError";
  }
}

async function call<T>(
  method: string,
  params: Record<string, unknown> = {},
  signal?: AbortSignal,
): Promise<T> {
  const id = requestId++;
  const resp = await fetch(RPC_PATH, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", method, params, id }),
    signal,
  });
  if (!resp.ok) {
    throw new RpcError(resp.status, `HTTP ${resp.status}: ${await resp.text()}`);
  }
  const json = (await resp.json()) as {
    result?: T;
    error?: { code: number; message: string };
  };
  if (json.error) throw new RpcError(json.error.code, json.error.message);
  return json.result as T;
}

// ============================================================================
// Response types
// ============================================================================

export interface BlockHeader {
  height: number;
  round: number;
  block_hash: string;
  parent_hash: string;
  state_root: string;
  tx_count: number;
}

export interface TxReceipt {
  block_height: number;
  tx_index: number;
  from: string;
  nonce: number;
  fee: number;
  payload_len: number;
}

export interface AiEntity {
  id: string;
  code_hash: string;
  creator: string;
  autonomy_mode: number;
  capabilities: number;
  economic_balance: string;
  nonce: number;
  pubkey: string;
  memory_root: string;
  params_root: string;
  registered_at: number;
  last_active_at: number;
  is_active: boolean;
}

export interface MemoryObject {
  object_id: string;
  object_type: number;
  owner_entity: string;
  created_at: number;
  updated_at: number;
  data: string;
  data_size: number;
}

export interface Signal {
  commitment_hash: string;
  signal_type: number;
  height: number;
  issuer: string;
}

export interface BalanceInfo {
  balance: string; // u128 as decimal string
  nonce: number;
}

// ============================================================================
// Method wrappers
// ============================================================================

export const rpc = {
  async getLatestBlock(signal?: AbortSignal): Promise<BlockHeader | null> {
    return call<BlockHeader | null>("novai_getLatestBlock", {}, signal);
  },
  async getBlockByHeight(
    height: number,
    signal?: AbortSignal,
  ): Promise<BlockHeader | null> {
    return call<BlockHeader | null>(
      "novai_getBlockByHeight",
      { height },
      signal,
    );
  },
  async getBlockByHash(
    hash: string,
    signal?: AbortSignal,
  ): Promise<BlockHeader | null> {
    return call<BlockHeader | null>("novai_getBlockByHash", { hash }, signal);
  },
  async getTransaction(
    txid: string,
    signal?: AbortSignal,
  ): Promise<TxReceipt | null> {
    return call<TxReceipt | null>("novai_getTransaction", { txid }, signal);
  },
  async getBalance(address: string, signal?: AbortSignal): Promise<BalanceInfo> {
    return call<BalanceInfo>("novai_getBalance", { address }, signal);
  },
  async getAiEntity(
    entityId: string,
    signal?: AbortSignal,
  ): Promise<AiEntity | null> {
    const result = await call<{ entity: AiEntity | null }>(
      "novai_getAiEntity",
      { entity_id: entityId },
      signal,
    );
    return result.entity;
  },
  async getMemoryObjects(
    entityId: string,
    signal?: AbortSignal,
  ): Promise<MemoryObject[]> {
    const result = await call<{ objects: MemoryObject[] }>(
      "novai_getMemoryObjects",
      { entity_id: entityId },
      signal,
    );
    return result.objects;
  },
  async getSignalsByIssuer(
    issuer: string,
    startHeight: number,
    endHeight: number,
    signal?: AbortSignal,
  ): Promise<Signal[]> {
    const result = await call<{ signals: Signal[] }>(
      "novai_getSignalsByIssuer",
      { issuer, start_height: startHeight, end_height: endHeight },
      signal,
    );
    return result.signals;
  },
};
