# tonic-otel-layer

Layer for a Tonic gRPC server that adds an OpenTelemetry metrics support.

## Usage

```rust
let exporter = MetricExporter::builder()
        .with_tonic()
        .build()?;

let meter_provider = SdkMeterProvider::builder()
        .with_resource(Resource::builder().with_service_name("my_service").build())
        .with_periodic_exporter(exporter)
        .build();

let metrics_layer = tonic_otel_layer::MetricsLayerBuilder::new()
        .with_provider(meter_provider)
        .build();

Server::builder()
    .layer(metrics_layer)
    .add_service(health_service)
    .serve(addr)
    .await?;
```

## Thanks to
[tonic-prometheus-layer](https://github.com/blkmlk/tonic-prometheus-layer) - layer for a tonic GRPC server (and client) which provides metrics in prometheus format.

[axum-otel-metrics](https://github.com/ttys3/axum-otel-metrics) - opentelemetry layer for an axum web-server.
