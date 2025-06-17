use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use opentelemetry::metrics::{Counter, Histogram, UpDownCounter};
use opentelemetry::{KeyValue, global};
use pin_project::pin_project;
use tonic::Code;
use tonic::codegen::http::{request, response};
use tower::{Layer, Service};

#[derive(Clone)]
pub struct MetricsLayer {
    metrics: Metrics,
}

#[derive(Clone)]
pub struct Metrics {
    pub started_total: Counter<u64>,
    pub handled_total: Counter<u64>,
    pub handling_duration: Histogram<f64>,
    pub active_requests: UpDownCounter<i64>,
}

const DEFAULT_HISTOGRAM_BUCKETS: [f64; 10] = [
    0.001, 0.005, 0.01, 0.015, 0.020, 0.025, 0.50, 0.75, 1.0, 2.0,
];

#[derive(Default)]
pub struct MetricsLayerBuilder {
    buckets: Option<Vec<f64>>,
    provider: Option<Arc<dyn opentelemetry::metrics::MeterProvider + Send + Sync>>,
}

impl MetricsLayerBuilder {
    pub fn new() -> Self {
        MetricsLayerBuilder::default()
    }
    pub fn with_buckets(mut self, buckets: Vec<f64>) -> Self {
        self.buckets = Some(buckets);
        self
    }

    pub fn with_provider<P>(mut self, provider: P) -> Self
    where
        P: opentelemetry::metrics::MeterProvider + Send + Sync + 'static,
    {
        self.provider = Some(Arc::new(provider));
        self
    }
    pub fn build(self) -> MetricsLayer {
        let provider = self.provider.unwrap_or_else(|| global::meter_provider());

        let meter = provider.meter("tonic");

        let buckets = self
            .buckets
            .unwrap_or_else(|| DEFAULT_HISTOGRAM_BUCKETS.to_vec());

        let started_total = meter
            .u64_counter("grpc_server_started")
            .with_description("Total number of RPCs started on the server.")
            .build();
        let handled_total = meter
            .u64_counter("grpc_server_handled")
            .with_description("Total number of RPCs completed on the server.")
            .build();
        let handling_duration = meter
            .f64_histogram("grpc_server_handling_duration_seconds")
            .with_description("Rpc call duration")
            .with_boundaries(buckets)
            .build();
        let active_requests = meter
            .i64_up_down_counter("grpc_server_active_requests")
            .with_description("Current number of active server requests.")
            .build();
        let metrics = Metrics {
            started_total,
            handled_total,
            handling_duration,
            active_requests,
        };
        MetricsLayer { metrics }
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            service: inner,
            metrics: self.metrics.clone(),
        }
    }
}

#[derive(Clone)]
pub struct MetricsService<S> {
    metrics: Metrics,
    service: S,
}

impl<S, B, C> Service<request::Request<B>> for MetricsService<S>
where
    S: Service<request::Request<B>, Response = response::Response<C>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = MetricsFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: request::Request<B>) -> Self::Future {
        let path = req.uri().path();
        let (service, method) = path.rsplit_once("/").expect("Path must contain a method");
        let service = service.to_owned();
        let method = method.to_owned();
        let metrics = self.metrics.clone();
        let inner = self.service.call(req);
        MetricsFuture {
            inner,
            metrics,
            service,
            method,
            started_at: None,
        }
    }
}

#[pin_project]
pub struct MetricsFuture<F> {
    #[pin]
    inner: F,
    metrics: Metrics,
    service: String,
    method: String,
    started_at: Option<Instant>,
}

impl<F, B, E> Future for MetricsFuture<F>
where
    F: Future<Output = Result<response::Response<B>, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let sm_labels = vec![
            KeyValue::new("grpc_service", this.service.clone()),
            KeyValue::new("grpc_method", this.method.clone()),
        ];

        let started_at = this.started_at.get_or_insert_with(|| {
            this.metrics.active_requests.add(1, &sm_labels);
            this.metrics.started_total.add(1, &sm_labels);
            Instant::now()
        });

        if let Poll::Ready(res) = this.inner.poll(cx) {
            let code = res.as_ref().map_or(Code::Unknown, |resp| {
                resp.headers()
                    .get("grpc-status")
                    .map(|s| Code::from_bytes(s.as_bytes()))
                    .unwrap_or(Code::Ok)
            });
            let smc_labels = [
                KeyValue::new("grpc_service", this.service.clone()),
                KeyValue::new("grpc_method", this.method.clone()),
                KeyValue::new("grpc_code", format!("{:?}", code)),
            ];
            let elapsed = started_at.elapsed().as_secs_f64();
            this.metrics.active_requests.add(-1, &sm_labels);
            this.metrics.handled_total.add(1, &smc_labels);
            this.metrics.handling_duration.record(elapsed, &smc_labels);

            Poll::Ready(res)
        } else {
            Poll::Pending
        }
    }
}
