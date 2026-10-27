/** Base58 identifier of an identity, contract, document or token. */
export type Id = string;
/** Amounts accept bigint, a decimal string, or a number up to 2^53. */
export type Amount = bigint | number | string;

/** Signing options shared by every write. */
export interface Signing {
  /** WIF or hex private key of one of the identity's keys. */
  privateKey: string;
  /** The identity key to sign with; by default, the enabled key `privateKey` belongs to. */
  keyId?: number;
}

/** Platform JSON for an identity, as serialized by dpp, with `balance` as a bigint. */
export interface Identity {
  id: Id;
  balance: bigint;
  revision: number;
  publicKeys: Array<Record<string, unknown>>;
  [field: string]: unknown;
}

/** Platform JSON for a document: `$id`, `$ownerId`, `$revision`, … plus its properties. */
export type Document = Record<string, unknown>;

/** evo-sdk style clauses: `[["field", "==", value]]`, `[["field", "asc"]]`. */
export type WhereClause = [string, string, unknown];
export type OrderClause = [string, 'asc' | 'desc'];

export interface DocumentQuery {
  dataContractId: Id;
  documentTypeName: string;
  where?: WhereClause[];
  orderBy?: OrderClause[];
  limit?: number;
  startAfter?: Id;
  startAt?: Id;
}

/** A secp256k1 public key, its hash160 and its P2PKH address on the connected network (hex). */
export interface DerivedKey {
  publicKey: string;
  publicKeyHash: string;
  address: string;
}

/** Token write results; balances are decimal strings. */
export type TokenResult = Record<string, unknown>;

export declare class DashPlatform {
  /** Connects this isolate, once; later calls return the same instance. One network per isolate. */
  static connect(options?: { network?: 'mainnet' | 'testnet' }): Promise<DashPlatform>;
  readonly network: 'mainnet' | 'testnet';
  /** Runs any method by name (e.g. "identities.fetch") with raw JSON results. */
  call(method: string, params?: Record<string, unknown>): Promise<unknown>;

  identities: {
    fetch(identityId: Id): Promise<Identity | null>;
    balance(identityId: Id): Promise<bigint | null>;
    balances(identityIds: Id[]): Promise<Record<Id, bigint | null>>;
    nonce(identityId: Id): Promise<bigint | null>;
    contractNonce(identityId: Id, contractId: Id): Promise<bigint | null>;
    /** `publicKeyHash` is 20 bytes of hex. */
    byPublicKeyHash(publicKeyHash: string): Promise<Identity | null>;
    /**
     * Registers an identity funded by an asset lock. `keys` are its secp256k1 keys in id order
     * and must start with a MASTER AUTHENTICATION key; purpose defaults to AUTHENTICATION and
     * securityLevel to HIGH.
     */
    create(opts: {
      /** Hex of the proof's standard bincode encoding, as evo-sdk's `AssetLockProof.toHex()`. */
      assetLockProof: string;
      assetLockPrivateKey: string;
      keys: Array<{
        privateKey: string;
        purpose?: 'AUTHENTICATION' | 'ENCRYPTION' | 'DECRYPTION' | 'TRANSFER' | 'VOTING' | 'OWNER';
        securityLevel?: 'MASTER' | 'CRITICAL' | 'HIGH' | 'MEDIUM';
      }>;
    }): Promise<Identity>;
    topUp(opts: { identityId: Id; assetLockProof: string; assetLockPrivateKey: string }): Promise<{
      balance: bigint;
    }>;
    /** Signs with a TRANSFER key. */
    creditTransfer(opts: Signing & { identityId: Id; recipientId: Id; amount: Amount }): Promise<{
      senderBalance: bigint;
      recipientBalance: bigint;
    }>;
    /** Signs with a TRANSFER key. Without `toAddress`, pays the identity's withdrawal address. */
    creditWithdrawal(opts: Signing & {
      identityId: Id;
      amount: Amount;
      toAddress?: string;
      coreFeePerByte?: number;
    }): Promise<{ balance: bigint }>;
  };

  dpns: {
    /** `name` like "alice.dash" or "alice". */
    resolveName(name: string): Promise<Id | null>;
    usernames(identityId: Id, options?: { limit?: number }): Promise<string[]>;
    isNameAvailable(name: string): Promise<boolean>;
    registerName(opts: Signing & { identityId: Id; label: string; contestFund?: Amount }): Promise<{
      name: string;
      domainDocument: Document;
    }>;
  };

  contracts: {
    fetch(contractId: Id): Promise<Record<string, unknown> | null>;
    fetchMany(contractIds: Id[]): Promise<Record<Id, Record<string, unknown> | null>>;
  };

  documents: {
    query(query: DocumentQuery): Promise<Document[]>;
    get(dataContractId: Id, documentTypeName: string, documentId: Id): Promise<Document | null>;
    create(opts: Signing & {
      ownerId: Id;
      dataContractId: Id;
      documentTypeName: string;
      data: Record<string, unknown>;
    }): Promise<Document>;
    /** Replaces the document's properties with `data` and bumps its revision. */
    replace(opts: Signing & {
      ownerId: Id;
      dataContractId: Id;
      documentTypeName: string;
      documentId: Id;
      data: Record<string, unknown>;
    }): Promise<Document>;
    delete(opts: Signing & {
      ownerId: Id;
      dataContractId: Id;
      documentTypeName: string;
      documentId: Id;
    }): Promise<{ deleted: Id }>;
    transfer(opts: Signing & {
      ownerId: Id;
      dataContractId: Id;
      documentTypeName: string;
      documentId: Id;
      recipientId: Id;
    }): Promise<Document>;
  };

  tokens: {
    identityBalances(identityId: Id, tokenIds: Id[]): Promise<Record<Id, bigint | null>>;
    identitiesBalances(tokenId: Id, identityIds: Id[]): Promise<Record<Id, bigint | null>>;
    totalSupply(tokenId: Id): Promise<{ totalSupply: bigint; totalBalances: bigint } | null>;
    statuses(tokenIds: Id[]): Promise<Record<Id, { paused: boolean } | null>>;
    contractInfo(tokenId: Id): Promise<{ contractId: Id; tokenPosition: number } | null>;
    transfer(opts: Signing & {
      identityId: Id;
      dataContractId: Id;
      tokenPosition: number;
      recipientId: Id;
      amount: Amount;
      publicNote?: string;
    }): Promise<TokenResult>;
    mint(opts: Signing & {
      identityId: Id;
      dataContractId: Id;
      tokenPosition: number;
      amount: Amount;
      recipientId?: Id;
      publicNote?: string;
    }): Promise<TokenResult>;
    burn(opts: Signing & {
      identityId: Id;
      dataContractId: Id;
      tokenPosition: number;
      amount: Amount;
      publicNote?: string;
    }): Promise<TokenResult>;
  };

  /** Core data from DAPI is not proof-verified: it is what the answering evonode reports. */
  core: {
    getTransaction(txid: string): Promise<{
      hex: string;
      height: number;
      isInstantLocked: boolean;
      isChainLocked: boolean;
    } | null>;
    broadcastTransaction(hex: string): Promise<{ txid: string }>;
  };

  assetLocks: {
    /**
     * Locks Core DASH into Platform credits for `assetLockPrivateKey`. Spends every output of
     * `fundingTxids` that pays `fundingPrivateKey`'s address; change goes back there. Without
     * `amount` (duffs), locks everything but the fee. Broadcasts, then waits for InstantSend.
     */
    create(opts: {
      fundingPrivateKey: string;
      fundingTxids: string[];
      assetLockPrivateKey: string;
      amount?: Amount;
      /** Sets the least amount Platform accepts: 200,000 duffs for "identity", 50,000 for "topUp". */
      use?: 'identity' | 'topUp';
    }): Promise<{
      txid: string;
      /** The signed transaction, to rebroadcast if needed. */
      hex: string;
      amount: bigint;
      change: bigint;
      changeAddress: string;
      /** An InstantSend proof if the lock arrived within 30 s, else null (use `proof` later). */
      proof: string | null;
    }>;
    /**
     * A chain proof (hex) once the transaction is chain-locked, else null. Platform may refuse
     * it for a block or two until its view of Core catches up; retry then.
     */
    proof(txid: string, options?: { outputIndex?: number }): Promise<string | null>;
  };

  keys: {
    /** A random secp256k1 key. Keep `privateKey` secret. */
    generate(): Promise<DerivedKey & { privateKey: string }>;
    derive(privateKey: string): Promise<DerivedKey>;
  };

  epoch: {
    current(): Promise<{
      index: number;
      firstBlockHeight: number;
      firstCoreBlockHeight: number;
      startTimeMs: number;
      feeMultiplierPermille: number;
      protocolVersion: number;
    }>;
  };

  system: {
    totalCredits(): Promise<bigint>;
  };
}
