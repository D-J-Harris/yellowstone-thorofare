# Yellowstone Thorofare

Benchmarking tool for Solana Geyser gRPC endpoints. Compares two endpoints simultaneously to measure actual performance differences.

## Build
```bash
cargo build --release
```

## What it measures

- All 6 slot stages: FirstShredReceived, CreatedBank, Completed, Processed, Confirmed, Finalized
- Download time (FirstShredReceived > Completed)
- Replay time (CreatedBank > Processed)
- Account update propagation delays (optional)
- Transaction propagation delays (optional)

## Basic usage
```bash
export X_TOKEN_TRITON=...   # token for the endpoint named "triton"
export X_TOKEN_HELIUS=...
thorofare --endpoint triton=https://triton.example:443 --endpoint helius=https://helius.example:443
```

## Arguments

Required:
- `--endpoint NAME=URL` - Endpoint to benchmark; repeat it for each endpoint (at least 2). URL is the full URL including scheme. The x-token is read from the environment variable `X_TOKEN_<NAME>` (NAME upper-cased, `-` turned to `_`), so it never appears in the command line, shell history or `ps`.

Optional:
- `--richat NAME` - Use the Richat interface for the endpoint with this NAME (repeatable)
- `--slots` - Number of slots to collect (default: 1000)
- `--with-accounts` - Track account updates; needs `--account-owner` or `--accounts-file`
- `--account-owner` - Filter accounts by program ID
- `--accounts-file PATH` - File of account addresses, one per line (`#` starts a comment). Each file is a separate account subscription on every endpoint, holding exactly those addresses. Repeatable. Not supported for Richat endpoints.
- `--commitment processed|confirmed|finalized` - Commitment of the account and transaction subscriptions (default: processed)
- `--with-transactions` - Track transaction updates (non-vote)
- `--transaction-account` - Only race transactions that touch this account; repeat for more (requires --with-transactions). Without it every non-vote transaction is streamed.
- `--core-slot-statuses` - Compare slots on CreatedBank, Processed, Confirmed and Finalized only. Use it for servers that never send FirstShredReceived or Completed, such as Firedancer Dragon's Mouth. Without it such slots are all dropped.
- `--config` - Config file (default: config.toml)
- `--output` - Output file (default: benchmark_results.json). Every pair of endpoints is compared on the same collected updates and written to `<stem>.<name1>-vs-<name2>.json`.
- `--log-level` - Log level (default: info)

## Examples

Three endpoints, accounts given by address in two subscriptions, at confirmed commitment:
```bash
thorofare \
  --endpoint triton=https://triton.example:443 \
  --endpoint helius=https://helius.example:443 \
  --endpoint quicknode=https://quicknode.example:10000 \
  --with-accounts \
  --accounts-file whirlpools.txt \
  --accounts-file other_pools.txt \
  --commitment confirmed
```

Track account updates by program owner:
```bash
thorofare \
  --endpoint a=https://endpoint1.com:10000 \
  --endpoint b=https://endpoint2.com:10000 \
  --with-accounts \
  --account-owner 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8
```

## Output

Generates JSON with:
- Slot timing comparisons
- P50/P90/P99 percentiles for all metrics
- Account update delays with win/loss counts (if `--with-accounts`)
- Transaction propagation delays with win/loss counts (if `--with-transactions`)
- Ping latencies

Delay metrics use 0ms for wins (this endpoint was faster) so the percentiles reflect the full population. A p50 of 0.00 means that endpoint was faster more than half the time.

You can visualize here https://thorofare.triton.one/

## Config

See config.toml for gRPC tuning options. Most important:
- `tcp_nodelay = true` for low latency
- `initial_connection_window_size` for heavy subscriptions
- `initial_connection_window_size` for heavy subscriptions
(Recommend reading: https://httpwg.org/specs/rfc9113.html#rfc.section.6.9.1, for window size)

## Contributing

PRs welcome. Keep it simple:
1. Fork and create a branch
2. Make your changes
3. Test against at least 2 different endpoints
4. Submit PR :D

## Related

- UI Repo: https://github.com/rpcpool/yellowstone-thorofare-ui
