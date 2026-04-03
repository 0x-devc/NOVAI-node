/**
 * JSON-RPC 2.0 client for communicating with a NOVAI node.
 */

import { encodeTxV1Signed, txidV1, bytesToHex } from "./encoding";
import { TxV1, AiEntityInfo, MemoryObjectInfo, SignalInfo } from "./types";

// Use dynamic import for http/https to avoid browser breakage.
// In Node.js 18+ we can also use global fetch.
let httpRequest: (
  url: string,
  body: string
) => Promise<{ status: number; body: string }>;

// Attempt to use global fetch (Node 18+ / browsers), fall back to http module
if (typeof globalThis.fetch === "function") {
  httpRequest = async (url: string, body: string) => {
    const resp = await globalThis.fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
    });
    const text = await resp.text();
    return { status: resp.status, body: text };
  };
} else {
  // Node.js < 18 fallback using http module
  httpRequest = (url: string, body: string) => {
    return new Promise((resolve, reject) => {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const http = require("http");
      const parsed = new URL(url);
      const req = http.request(
        {
          hostname: parsed.hostname,
          port: parsed.port,
          path: parsed.pathname,
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Content-Length": Buffer.byteLength(body),
          },
        },
        (res: { statusCode: number; on: Function }) => {
          let data = "";
          res.on("data", (chunk: string) => (data += chunk));
          res.on("end", () =>
            resolve({ status: res.statusCode, body: data })
          );
        }
      );
      req.on("error", reject);
      req.write(body);
      req.end();
    });
  };
}

interface RpcResponse {
  jsonrpc: string;
  result?: unknown;
  error?: { code: number; message: string };
  id: number;
}

let requestId = 1;

/** NOVAI RPC client. */
export class NovaiClient {
  private endpoint: string;

  constructor(endpoint: string = "http://localhost:3030") {
    this.endpoint = endpoint;
  }

  /** Send a raw JSON-RPC call. */
  async call(method: string, params: Record<string, unknown>): Promise<unknown> {
    const id = requestId++;
    const body = JSON.stringify({
      jsonrpc: "2.0",
      method,
      params,
      id,
    });

    const resp = await httpRequest(this.endpoint, body);

    if (resp.status !== 200) {
      throw new Error(`HTTP ${resp.status}: ${resp.body}`);
    }

    const rpcResp: RpcResponse = JSON.parse(resp.body);

    if (rpcResp.error) {
      throw new Error(
        `RPC error ${rpcResp.error.code}: ${rpcResp.error.message}`
      );
    }

    return rpcResp.result;
  }

  /** Submit a signed transaction. Returns the hex-encoded txid. */
  async submitTx(tx: TxV1): Promise<string> {
    const bytes = encodeTxV1Signed(tx);
    const hexStr = bytesToHex(bytes);
    const result = (await this.call("novai_submitTransaction", {
      tx: hexStr,
    })) as { txid: string };
    return result.txid;
  }

  /** Query the expected nonce for an address (hex-encoded). */
  async getNonce(addressHex: string): Promise<bigint> {
    const result = (await this.call("novai_getNonce", {
      address: addressHex,
    })) as { nonce: number };
    return BigInt(result.nonce);
  }

  /** Query account balance and nonce. */
  async getBalance(
    addressHex: string
  ): Promise<{ balance: string; nonce: bigint }> {
    const result = (await this.call("novai_getBalance", {
      address: addressHex,
    })) as { balance: string; nonce: number };
    return { balance: result.balance, nonce: BigInt(result.nonce) };
  }

  /** Query AI entity state. Returns null if entity not found. */
  async getAiEntity(entityIdHex: string): Promise<AiEntityInfo | null> {
    const result = (await this.call("novai_getAiEntity", {
      entity_id: entityIdHex,
    })) as { entity: AiEntityInfo | null };
    return result.entity;
  }

  /** Query memory objects for an entity. */
  async getMemoryObjects(entityIdHex: string): Promise<MemoryObjectInfo[]> {
    const result = (await this.call("novai_getMemoryObjects", {
      entity_id: entityIdHex,
    })) as { objects: MemoryObjectInfo[] };
    return result.objects;
  }

  /** Request tokens from the faucet (dev mode only). */
  async faucet(
    addressHex: string
  ): Promise<{ txid: string; amount: string }> {
    const result = (await this.call("novai_faucet", {
      address: addressHex,
    })) as { txid: string; amount: string };
    return result;
  }

  /** Query signals at a specific block height. */
  async getSignalsByHeight(height: number): Promise<SignalInfo[]> {
    const result = (await this.call("novai_getSignalsByHeight", {
      height,
    })) as { signals: SignalInfo[] };
    return result.signals;
  }

  /** Query signals by issuer within a height range. */
  async getSignalsByIssuer(
    issuerHex: string,
    startHeight: number,
    endHeight: number
  ): Promise<SignalInfo[]> {
    const result = (await this.call("novai_getSignalsByIssuer", {
      issuer: issuerHex,
      start_height: startHeight,
      end_height: endHeight,
    })) as { signals: SignalInfo[] };
    return result.signals;
  }

  /** Query signals by type within a height range. */
  async getSignalsByType(
    signalType: number,
    startHeight: number,
    endHeight: number
  ): Promise<SignalInfo[]> {
    const result = (await this.call("novai_getSignalsByType", {
      signal_type: signalType,
      start_height: startHeight,
      end_height: endHeight,
    })) as { signals: SignalInfo[] };
    return result.signals;
  }

  /** Compute the txid locally without submitting. */
  computeTxId(tx: TxV1): string {
    return bytesToHex(txidV1(tx));
  }
}
