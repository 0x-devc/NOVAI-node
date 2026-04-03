/** 32-byte address (blake3 hash of public key). */
export type Address = Uint8Array;

/** 32-byte transaction ID. */
export type TxId = Uint8Array;

/** 32-byte hash. */
export type Hash32 = Uint8Array;

/** 64-byte Ed25519 signature. */
export type SignatureBytes = Uint8Array;

/** Keypair with 32-byte seed, public key, and derived address. */
export interface Keypair {
  /** 32-byte Ed25519 seed (private). */
  seed: Uint8Array;
  /** 32-byte Ed25519 public key. */
  publicKey: Uint8Array;
  /** 32-byte NOVAI address. */
  address: Uint8Array;
}

/** Unsigned transaction fields. */
export interface TxV1 {
  version: number;
  from: Uint8Array;
  pubkey: Uint8Array;
  nonce: bigint;
  fee: bigint;
  payload: Uint8Array;
  sig: Uint8Array;
}

/** Autonomy mode for AI entities. */
export enum AutonomyMode {
  Advisory = 0,
  Gated = 1,
}

/** Signal types for AI signal commitments. */
export enum SignalType {
  Anomaly = 0,
  Optimization = 1,
  Prediction = 2,
  RiskScore = 3,
  AuditReport = 4,
  SpamRisk = 5,
  CongestionForecast = 6,
}

/** Memory object types. */
export enum MemoryObjectType {
  ChainSummary = 0,
  LabelIndex = 1,
  EmbeddingCommitment = 2,
  AnomalyLog = 3,
  StatisticsSnapshot = 4,
}

/** Capability flags (bitfield). */
export interface Capabilities {
  readPublicChain?: boolean;
  readMemoryObjects?: boolean;
  emitProposals?: boolean;
  requestExecution?: boolean;
  readNnpxDerived?: boolean;
}

/** AI entity info returned from RPC. */
export interface AiEntityInfo {
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

/** Memory object info returned from RPC. */
export interface MemoryObjectInfo {
  object_id: string;
  object_type: number;
  owner_entity: string;
  created_at: number;
  updated_at: number;
  data: string;
  data_size: number;
}

/** Signal commitment info returned from RPC. */
export interface SignalInfo {
  commitment_hash: string;
  signal_type: number;
  height: number;
  issuer: string;
}
