use {
    crate::{
        Config, GrpcConfig,
        grpc::{GrpcError, SlotCollector, Subscriptions},
        processor::{EndpointMetadata, GrpcConfigSummary},
        types::EndpointData,
    },
    futures::future::join_all,
    std::{sync::Arc, time::Duration},
    tokio::sync::Barrier,
    tracing::info,
};

pub struct EndpointSpec {
    pub name: String,
    pub url: String,
    pub x_token: Option<String>,
    pub richat: bool,
}

pub struct CollectedEndpoint {
    pub name: String,
    pub data: EndpointData,
    pub meta: EndpointMetadata,
    pub avg_ping: Duration,
}

pub struct Collector {
    config: Config,
    endpoints: Vec<EndpointSpec>,
    slot_count: usize,
    subscriptions: Subscriptions,
}

impl Collector {
    pub const fn new(
        config: Config,
        endpoints: Vec<EndpointSpec>,
        slot_count: usize,
        subscriptions: Subscriptions,
    ) -> Self {
        Self {
            config,
            endpoints,
            slot_count,
            subscriptions,
        }
    }

    pub async fn run(self) -> Result<Vec<CollectedEndpoint>, GrpcError> {
        info!("Collecting endpoint versions and ping averages...");

        let mut collectors = Vec::with_capacity(self.endpoints.len());
        for endpoint in &self.endpoints {
            let collector = SlotCollector::new(
                self.make_grpc_config(endpoint.url.clone(), endpoint.x_token.clone()),
                self.slot_count,
                self.config.benchmark.buffer_percentage,
                self.config.benchmark.latency_samples,
                self.subscriptions.clone(),
                endpoint.richat,
            )
            .await?;
            collectors.push(collector);
        }

        let extras = match (
            self.subscriptions.with_accounts,
            self.subscriptions.with_transactions,
        ) {
            (true, true) => " with account and transaction updates",
            (true, false) => " with account updates",
            (false, true) => " with transaction updates",
            (false, false) => "",
        };
        info!("Starting synchronized collection{}", extras);

        let barrier = Arc::new(Barrier::new(collectors.len()));
        let mut metadata = Vec::with_capacity(collectors.len());
        let mut runs = Vec::with_capacity(collectors.len());
        for (endpoint, collector) in self.endpoints.iter().zip(collectors) {
            let plugin_type = match endpoint.richat {
                true => "Richat",
                false => "Yellowstone",
            };
            let meta = EndpointMetadata {
                plugin_type: plugin_type.to_string(),
                plugin_version: collector.version.clone(),
            };
            metadata.push((endpoint.name.clone(), meta, collector.avg_ping));
            runs.push(Self::collect_with_barrier(collector, Arc::clone(&barrier)));
        }

        let collected = join_all(runs).await;

        let mut endpoints = Vec::with_capacity(collected.len());
        for ((name, meta, avg_ping), data) in metadata.into_iter().zip(collected) {
            let data = data?;
            if self.subscriptions.with_accounts {
                info!(
                    "Collected {} account updates from {}",
                    data.account_updates.len(),
                    name
                );
            }
            if self.subscriptions.with_transactions {
                info!(
                    "Collected {} transaction updates from {}",
                    data.transaction_updates.len(),
                    name
                );
            }
            endpoints.push(CollectedEndpoint {
                name,
                data,
                meta,
                avg_ping,
            });
        }

        Ok(endpoints)
    }

    async fn collect_with_barrier(
        collector: SlotCollector,
        barrier: Arc<Barrier>,
    ) -> Result<EndpointData, GrpcError> {
        barrier.wait().await;
        collector.collect().await
    }

    pub const fn get_grpc_config_summary(&self) -> GrpcConfigSummary {
        let g = &self.config.grpc;
        GrpcConfigSummary {
            connect_timeout_ms: g.connect_timeout.as_millis() as u64,
            request_timeout_ms: g.request_timeout.as_millis() as u64,
            max_message_size: g.max_message_size,
            use_tls: g.use_tls,
            http2_adaptive_window: g.http2_adaptive_window,
            initial_connection_window_size: g.initial_connection_window_size,
            initial_stream_window_size: g.initial_stream_window_size,
        }
    }

    const fn make_grpc_config(&self, endpoint: String, x_token: Option<String>) -> GrpcConfig {
        let g = &self.config.grpc;
        GrpcConfig {
            endpoint,
            x_token,
            connect_timeout: g.connect_timeout,
            request_timeout: g.request_timeout,
            max_message_size: g.max_message_size,
            use_tls: g.use_tls,
            http2_adaptive_window: g.http2_adaptive_window,
            http2_keep_alive_interval: g.http2_keep_alive_interval,
            initial_connection_window_size: g.initial_connection_window_size,
            initial_stream_window_size: g.initial_stream_window_size,
            tcp_nodelay: g.tcp_nodelay,
            tcp_keepalive: g.tcp_keepalive,
            buffer_size: g.buffer_size,
        }
    }
}
