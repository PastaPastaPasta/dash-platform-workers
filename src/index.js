// Dash Platform for Cloudflare Workers. Names follow evo-sdk; every read is proof-verified.
import * as wasm from '../dist/wasm/dash_platform.js';
import wasmModule from '../dist/wasm/dash_platform_bg.wasm';
// Routes the SDK's requests to evonode IPs over TLS sockets; see ip-fetch.js.
import './ip-fetch.js';

// Instantiating at module scope bills compilation to Worker startup, not to the first request.
wasm.initSync({ module: wasmModule });

const big = (v) => (v == null ? null : BigInt(v));
const bigValues = (obj) => Object.fromEntries(Object.entries(obj).map(([k, v]) => [k, big(v)]));
const withBigBalance = (identity) => identity && { ...identity, balance: big(identity.balance) };
// BigInt amounts go to Rust as decimal strings; JSON has no 64-bit integers.
const replacer = (_, v) => (typeof v === 'bigint' ? v.toString() : v);

async function call(method, params = {}) {
  return JSON.parse(await wasm.call(method, JSON.stringify(params, replacer)));
}

let connected;

export class DashPlatform {
  /**
   * Connects this isolate to `network` and loads quorum keys; later calls reuse the connection.
   * One network per isolate.
   */
  static async connect({ network = 'mainnet' } = {}) {
    if (connected && connected.network !== network) {
      throw new Error(`this isolate is connected to ${connected.network}; one network per isolate`);
    }
    if (!connected) {
      const ready = wasm.connect(network);
      connected = { network, ready };
      ready.catch(() => { connected = undefined; });
    }
    await connected.ready;
    return new DashPlatform(network);
  }

  constructor(network) {
    this.network = network;
    /** Runs any method by its Rust name with raw JSON results (amounts stay decimal strings). */
    this.call = call;

    this.identities = {
      fetch: async (identityId) => withBigBalance(await call('identities.fetch', { identityId })),
      balance: async (identityId) => big(await call('identities.balance', { identityId })),
      balances: async (identityIds) => bigValues(await call('identities.balances', { identityIds })),
      nonce: async (identityId) => big(await call('identities.nonce', { identityId })),
      contractNonce: async (identityId, contractId) =>
        big(await call('identities.contractNonce', { identityId, contractId })),
      byPublicKeyHash: async (publicKeyHash) =>
        withBigBalance(await call('identities.byPublicKeyHash', { publicKeyHash })),
      create: async (opts) => withBigBalance(await call('identities.create', opts)),
      topUp: async (opts) => bigValues(await call('identities.topUp', opts)),
      creditTransfer: async (opts) => bigValues(await call('identities.creditTransfer', opts)),
      creditWithdrawal: async (opts) => bigValues(await call('identities.creditWithdrawal', opts)),
    };

    this.dpns = {
      resolveName: (name) => call('dpns.resolveName', { name }),
      usernames: (identityId, { limit } = {}) => call('dpns.usernames', { identityId, limit }),
      isNameAvailable: (name) => call('dpns.isNameAvailable', { name }),
      registerName: (opts) => call('dpns.registerName', opts),
    };

    this.contracts = {
      fetch: (contractId) => call('contracts.fetch', { contractId }),
      fetchMany: (contractIds) => call('contracts.fetchMany', { contractIds }),
    };

    this.documents = {
      query: (opts) => call('documents.query', opts),
      get: (dataContractId, documentTypeName, documentId) =>
        call('documents.get', { dataContractId, documentTypeName, documentId }),
      create: (opts) => call('documents.create', opts),
      replace: (opts) => call('documents.replace', opts),
      delete: (opts) => call('documents.delete', opts),
      transfer: (opts) => call('documents.transfer', opts),
    };

    this.tokens = {
      identityBalances: async (identityId, tokenIds) =>
        bigValues(await call('tokens.identityBalances', { identityId, tokenIds })),
      identitiesBalances: async (tokenId, identityIds) =>
        bigValues(await call('tokens.identitiesBalances', { tokenId, identityIds })),
      totalSupply: async (tokenId) => {
        const supply = await call('tokens.totalSupply', { tokenId });
        return supply && bigValues(supply);
      },
      statuses: (tokenIds) => call('tokens.statuses', { tokenIds }),
      contractInfo: (tokenId) => call('tokens.contractInfo', { tokenId }),
      transfer: (opts) => call('tokens.transfer', opts),
      mint: (opts) => call('tokens.mint', opts),
      burn: (opts) => call('tokens.burn', opts),
    };

    this.core = {
      getTransaction: (txid) => call('core.getTransaction', { txid }),
      broadcastTransaction: (hex) => call('core.broadcastTransaction', { hex }),
    };

    this.assetLocks = {
      create: async (opts) => {
        const r = await call('assetLocks.create', opts);
        return { ...r, amount: big(r.amount), change: big(r.change) };
      },
      proof: (txid, { outputIndex } = {}) => call('assetLocks.proof', { txid, outputIndex }),
    };

    this.keys = {
      /** A new random secp256k1 private key (hex) with its public key, hash and address. */
      generate: async () => {
        const privateKey = [...crypto.getRandomValues(new Uint8Array(32))]
          .map((b) => b.toString(16).padStart(2, '0'))
          .join('');
        return { privateKey, ...(await call('keys.derive', { privateKey })) };
      },
      derive: (privateKey) => call('keys.derive', { privateKey }),
    };

    this.epoch = {
      current: () => call('epoch.current'),
    };

    this.system = {
      totalCredits: async () => big(await call('system.totalCredits')),
    };
  }
}
