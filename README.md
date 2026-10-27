# dash-platform-workers

Proof-verified Dash Platform reads and writes from Cloudflare Workers, small and cheap enough
for the **Free plan**.

```js
import { DashPlatform } from 'dash-platform-workers';

export default {
  async fetch() {
    const dash = await DashPlatform.connect({ network: 'mainnet' });
    const id = await dash.dpns.resolveName('quantum.dash');
    return Response.json({ id, balance: String(await dash.identities.balance(id)) });
  },
};
```

Every read is verified against a Platform proof signed by the current quorum, the same way the
Rust SDK verifies it; nothing is trusted from the node that answered.

## Why not evo-sdk?

[`@dashevo/evo-sdk`](https://www.npmjs.com/package/@dashevo/evo-sdk) runs in Workers, but it
does not fit the Free plan and needs workarounds even on Paid:

- **Size.** evo-sdk uploads at about 7 MB gzipped; the Free plan allows 3 MiB. Every exported
  wasm-bindgen function keeps its code alive, so feature flags cannot shrink it enough. This
  package builds a curated surface on `dash-sdk` (the Rust SDK) behind one exported entry
  point, at about 1.8 MB gzipped.
- **Reaching evonodes.** Workers' `fetch()` refuses bare-IP URLs (error 1003), and evonodes are
  addressed by IP. Their certificates carry the IP, so this package sends DAPI's gRPC-web over a
  TLS socket (`cloudflare:sockets`) instead; see [`src/ip-fetch.js`](src/ip-fetch.js).
- **CPU.** One BLS signature check costs about 20 ms of CPU in a Worker. Proved responses from
  the same block share a signature, and the Platform fix that checks it once
  ([dashpay/platform#5610](https://github.com/dashpay/platform/pull/5610)) brings a warm proved
  query to 3–10 ms of CPU. Until it merges, this package builds against that branch.

## API

Names follow evo-sdk. Credit and token amounts are `bigint` in results and accept `bigint`,
number or decimal string in arguments. See [`src/index.d.ts`](src/index.d.ts) for every
signature.

| Group | Reads | Writes |
| --- | --- | --- |
| `identities` | `fetch`, `balance`, `balances`, `nonce`, `contractNonce`, `byPublicKeyHash` | `create`, `topUp`, `creditTransfer`, `creditWithdrawal` |
| `dpns` | `resolveName`, `usernames`, `isNameAvailable` | `registerName` |
| `contracts` | `fetch`, `fetchMany` | |
| `documents` | `query`, `get` | `create`, `replace`, `delete`, `transfer` |
| `tokens` | `identityBalances`, `identitiesBalances`, `totalSupply`, `statuses`, `contractInfo` | `transfer`, `mint`, `burn` |
| `epoch`, `system` | `current`, `totalCredits` | |
| `core` | `getTransaction` | `broadcastTransaction` |
| `assetLocks` | `proof` | `create` |
| `keys` | `generate`, `derive` | |

Writes take `privateKey` (WIF or hex) and optionally `keyId`. Without `keyId`, the identity key
that `privateKey` belongs to is used. Keep keys in [Worker secrets](https://developers.cloudflare.com/workers/configuration/secrets/).

## Things to know

- **Free plan CPU.** Cloudflare documents 10 ms of CPU per request on the Free plan. Measured on
  a Free account, it is enforced as a burst budget rather than a hard per-request cap: requests
  of 20–200 ms succeed, and an isolate is cut off past roughly 2 s. The first request on a new
  isolate costs 70–200 ms (quorum keys, lazy compilation, the first BLS check); warm proved reads
  cost 3–10 ms. Writes wait for a proof from a new block, so each pays one full BLS check.
- **Concurrent writes from one identity.** Identity nonces are per identity, and Cloudflare runs
  many isolates. Send writes for one identity through a single
  [Durable Object](https://developers.cloudflare.com/durable-objects/) so they are serialized.
- **One network per isolate.** `connect()` sets up one network (quorum keys are fetched as
  proofs need them); connecting the same isolate to another network throws.
- **Trust.** Platform proofs are checked against quorum public keys from Dash Core Group's
  quorum service (`quorums.<network>.networks.dash.org`), as in evo-sdk's "trusted" mode. Core
  chain data (`core.*`) is not proved: it is what the answering evonode reports, though a
  funding transaction must hash to the txid asked for.
- **Asset locks.** `assetLocks.create` spends every output of `fundingTxids` that pays the
  funding key, waits up to 30 s for InstantSend and returns a ready proof; otherwise use
  `assetLocks.proof(txid)` once the block is chain-locked. It refuses amounts below Platform's
  minimum (200,000 duffs to create an identity, 50,000 to top one up), which would be stranded.
- **Panics.** The module aborts on a Rust panic; that isolate then refuses further calls with
  an error, and Cloudflare replaces it.

## Building

Needs Rust (pinned in `rust-toolchain.toml`) with the `wasm32-unknown-unknown` target, `clang`
and `llvm-ar` that can target wasm32 (on macOS: `brew install llvm`), `protoc`,
`wasm-bindgen` 0.2.108 and `wasm-opt`.

```sh
scripts/build.sh                                  # writes dist/wasm, fails above 2.8 MiB gzipped
PLATFORM_DIR=~/src/platform scripts/build.sh      # against a local dashpay/platform checkout
npm test
```

Try it locally with `npx wrangler dev` in [`examples/basic`](examples/basic).
