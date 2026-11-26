use {
    clap::{Parser, ValueEnum},
    solana_pubkey::Pubkey,
    std::{collections::HashSet, fs, str::FromStr, time::Instant},
    tracing::{error, info},
    yellowstone_thorofare::{
        BenchmarkResult, Collector, Config, EndpointSpec, EndpointSummary, Percentiles, Processor,
        Subscriptions,
    },
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[clap(name = "grpc-bench", about = "Benchmark gRPC endpoints")]
struct Args {
    /// Endpoint to benchmark, as NAME=URL; repeat for each endpoint (at least 2). The x-token is
    /// read from the environment variable X_TOKEN_<NAME>, NAME upper-cased and '-' turned to '_'.
    #[clap(long = "endpoint", required = true)]
    endpoints: Vec<String>,

    /// Use the Richat interface for the endpoint with this NAME (repeatable)
    #[clap(long = "richat")]
    richat: Vec<String>,

    /// Number of slots to collect
    #[clap(short, long, default_value = "1000")]
    slots: usize,

    /// Config file path
    #[clap(short, long, default_value = "config.toml")]
    config: String,

    /// Output JSON file; each pair of endpoints writes <stem>.<name1>-vs-<name2>.json
    #[clap(long, default_value = "benchmark_results.json")]
    output: String,

    /// Log level
    #[clap(long, default_value = "info")]
    log_level: String,

    /// Collect account updates for comparison (needs --account-owner or --accounts-file)
    #[clap(long)]
    with_accounts: bool,

    /// Filter accounts by owner pubkey
    #[clap(long, conflicts_with = "accounts_files")]
    account_owner: Option<String>,

    /// File of account addresses, one per line ('#' starts a comment); each file is its own
    /// account subscription on every endpoint (repeatable)
    #[clap(long = "accounts-file")]
    accounts_files: Vec<String>,

    /// Commitment of the account and transaction subscriptions
    #[clap(long, value_enum, default_value = "processed")]
    commitment: Commitment,

    /// Collect all transaction updates for comparison
    #[clap(long)]
    with_transactions: bool,

    /// Only race transactions that touch this account (repeatable, requires --with-transactions)
    #[clap(long = "transaction-account")]
    transaction_accounts: Vec<String>,

    /// Compare slots on CreatedBank, Processed, Confirmed and Finalized only, for servers that
    /// never send FirstShredReceived or Completed (Firedancer Dragon's Mouth)
    #[clap(long)]
    core_slot_statuses: bool,
}

/// Values follow Yellowstone's `CommitmentLevel`.
#[derive(Clone, Copy, ValueEnum)]
enum Commitment {
    Processed = 0,
    Confirmed = 1,
    Finalized = 2,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let filter = args
        .log_level
        .parse::<tracing_subscriber::filter::LevelFilter>()
        .unwrap_or(tracing_subscriber::filter::LevelFilter::INFO);

    tracing_subscriber::fmt()
        .with_max_level(filter)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();

    let config = match Config::load(&args.config) {
        Ok(cfg) => {
            info!("Loaded config from {}", args.config);
            cfg
        }
        Err(_) => {
            info!("Using default config");
            Config::default()
        }
    };

    info!("Starting Yellowstone-Thorofare v{}", VERSION);

    let endpoints = parse_endpoints(&args.endpoints, &args.richat);

    let has_account_filter = args.account_owner.is_some() || !args.accounts_files.is_empty();
    if args.with_accounts != has_account_filter {
        panic!("--with-accounts needs --account-owner or --accounts-file, and they need it");
    }
    if let Some(ref owner) = args.account_owner {
        if Pubkey::from_str(owner).is_err() {
            panic!("Invalid account owner pubkey: {}", owner);
        }
        info!("Account Owner Filter: {}", owner);
    }
    let account_lists: Vec<Vec<String>> = args
        .accounts_files
        .iter()
        .map(|path| read_accounts_file(path))
        .collect();
    let is_richat_with_account_list =
        !account_lists.is_empty() && endpoints.iter().any(|endpoint| endpoint.richat);
    if is_richat_with_account_list {
        panic!("--accounts-file is not supported for Richat endpoints, which stream every account");
    }
    for (path, accounts) in args.accounts_files.iter().zip(&account_lists) {
        info!(
            "Account subscription from {}: {} accounts",
            path,
            accounts.len()
        );
    }

    if !args.transaction_accounts.is_empty() && !args.with_transactions {
        panic!("--transaction-account requires --with-transactions");
    }
    for account in &args.transaction_accounts {
        if Pubkey::from_str(account).is_err() {
            panic!("Invalid transaction account pubkey: {}", account);
        }
    }
    if !args.transaction_accounts.is_empty() {
        info!(
            "Transaction Account Filter: {:?}",
            args.transaction_accounts
        );
    }
    info!("With Accounts: {}", args.with_accounts);
    info!("With Transactions: {}", args.with_transactions);
    info!("Core slot statuses only: {}", args.core_slot_statuses);
    for endpoint in &endpoints {
        let plugin = if endpoint.richat {
            "Richat"
        } else {
            "Yellowstone"
        };
        let token = if endpoint.x_token.is_some() {
            "x-token set"
        } else {
            "no x-token"
        };
        info!(
            "Endpoint {}: {} ({}, {})",
            endpoint.name, endpoint.url, plugin, token
        );
    }
    info!("Target slots: {}", args.slots);

    let start_time = Instant::now();

    let subscriptions = Subscriptions {
        with_accounts: args.with_accounts,
        account_owner: args.account_owner.clone(),
        account_lists,
        with_transactions: args.with_transactions,
        transaction_accounts: args.transaction_accounts.clone(),
        commitment: args.commitment as i32,
    };
    let collector = Collector::new(config, endpoints, args.slots, subscriptions);

    info!("Starting data collection...");

    let grpc_config_summary = collector.get_grpc_config_summary();

    let collected = match collector.run().await {
        Ok(collected) => collected,
        Err(e) => {
            error!("Benchmark failed: {}", e);
            std::process::exit(1);
        }
    };
    info!("Collection complete, processing results...");

    let stem = args.output.strip_suffix(".json").unwrap_or(&args.output);
    for (i, first) in collected.iter().enumerate() {
        for second in &collected[i + 1..] {
            // ponytail: clones each endpoint's updates once per pair; make Processor borrow if
            // many endpoints with long account runs run out of memory.
            let result = Processor::process(
                VERSION.to_string(),
                args.with_accounts,
                args.account_owner.clone(),
                args.with_transactions,
                args.transaction_accounts.clone(),
                args.core_slot_statuses,
                grpc_config_summary.clone(),
                first.data.clone(),
                second.data.clone(),
                first.meta.clone(),
                second.meta.clone(),
                first.avg_ping,
                second.avg_ping,
                start_time,
            );

            let output = format!("{}.{}-vs-{}.json", stem, first.name, second.name);
            let json = serde_json::to_string_pretty(&result).expect("Failed to serialize result");
            fs::write(&output, json).expect("Unable to write output file");
            info!("Results saved to {}", output);

            log_result(&result, &first.name, &second.name);
        }
    }
}

/// Parses NAME=URL specs and reads each x-token from X_TOKEN_<NAME>.
fn parse_endpoints(specs: &[String], richat: &[String]) -> Vec<EndpointSpec> {
    if specs.len() < 2 {
        panic!("Give at least 2 --endpoint NAME=URL");
    }
    let mut names = HashSet::new();
    let endpoints: Vec<EndpointSpec> = specs
        .iter()
        .map(|spec| {
            let Some((name, url)) = spec.split_once('=') else {
                panic!("--endpoint must be NAME=URL, got {}", spec);
            };
            let is_valid_name = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !is_valid_name {
                panic!(
                    "Endpoint name must be letters, digits, '-' or '_', got {}",
                    name
                );
            }
            if !names.insert(name.to_string()) {
                panic!("Endpoint name {} given twice", name);
            }
            let token_var = format!("X_TOKEN_{}", name.to_ascii_uppercase().replace('-', "_"));
            EndpointSpec {
                name: name.to_string(),
                url: url.to_string(),
                x_token: std::env::var(token_var)
                    .ok()
                    .filter(|token| !token.is_empty()),
                richat: richat.iter().any(|richat_name| richat_name == name),
            }
        })
        .collect();
    for name in richat {
        if !names.contains(name) {
            panic!("--richat {} names no endpoint", name);
        }
    }

    endpoints
}

fn read_accounts_file(path: &str) -> Vec<String> {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Unable to read accounts file {}: {}", path, e));
    let accounts: Vec<String> = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(|account| {
            if Pubkey::from_str(account).is_err() {
                panic!("Invalid account pubkey in {}: {}", path, account);
            }
            account.to_string()
        })
        .collect();
    if accounts.is_empty() {
        panic!("Accounts file {} has no accounts", path);
    }

    accounts
}

fn log_result(result: &BenchmarkResult, first: &str, second: &str) {
    info!("\n=== BENCHMARK SUMMARY: {} vs {} ===", first, second);
    info!("Tool version: {}", VERSION);
    info!(
        "Total slots collected: {}",
        result.metadata.total_slots_collected
    );
    info!("Common slots: {}", result.metadata.common_slots);
    info!("Compared slots: {}", result.metadata.compared_slots);
    info!("Dropped slots: {}", result.metadata.dropped_slots);
    info!("Duration: {}ms", result.metadata.duration_ms);

    for (name, endpoint) in [first, second].iter().zip(&result.endpoints) {
        info!(
            "\nEndpoint {}: {} ({} v{}) (ping: {:.2}ms)",
            name,
            endpoint.endpoint,
            endpoint.plugin_type,
            endpoint.plugin_version,
            endpoint.avg_ping_ms
        );
        info!("  Total updates: {}", endpoint.total_updates);
        info!("  Unique slots: {}", endpoint.unique_slots);
    }

    log_summary(first, &result.endpoint1_summary);
    log_summary(second, &result.endpoint2_summary);
}

fn log_summary(name: &str, summary: &EndpointSummary) {
    info!("\n=== {} PERFORMANCE (ms) ===", name);
    log_percentiles("First Shred Delay", Some(&summary.first_shred_delay));
    log_percentiles("Processing Delay", Some(&summary.processing_delay));
    log_percentiles("Confirmation Delay", Some(&summary.confirmation_delay));
    log_percentiles("Finalization Delay", Some(&summary.finalization_delay));
    log_percentiles("Download Time", Some(&summary.download_time));
    log_percentiles("Replay Time", Some(&summary.replay_time));
    log_percentiles("Confirmation Time", Some(&summary.confirmation_time));
    log_percentiles("Finalization Time", Some(&summary.finalization_time));
    log_percentiles("Account Delay", summary.account_delay.as_ref());
    log_wins("Account", summary.account_faster, summary.account_matched);
    log_percentiles("Transaction Delay", summary.transaction_delay.as_ref());
    log_wins(
        "Transaction",
        summary.transaction_faster,
        summary.transaction_matched,
    );
}

fn log_percentiles(label: &str, percentiles: Option<&Percentiles>) {
    let (p50, p90, p99) = percentiles.map_or((0.0, 0.0, 0.0), |p| (p.p50, p.p90, p.p99));
    info!("{}: p50={:.2}, p90={:.2}, p99={:.2}", label, p50, p90, p99);
}

fn log_wins(label: &str, faster: u64, matched: u64) {
    if matched == 0 {
        return;
    }
    let pct = faster as f64 / matched as f64 * 100.0;
    info!("{} Wins: {}/{} ({:.1}%)", label, faster, matched, pct);
}
