use crate::config::DOCKER_IMAGE;
use hawkeye_core::models::Status;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Service};
use serde_json::json;

/// Builds an idempotent name for the `ConfigMap` based on the `watcher_id`.
pub fn configmap_name(watcher_id: &str) -> String {
    format!("hawkeye-config-{}", watcher_id)
}

/// Builds a `ConfigMap` in the format expected to run the hawkeye-worker.
pub fn build_configmap(watcher_id: &str, contents: &str) -> ConfigMap {
    serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {
            "name": configmap_name(watcher_id),
            "labels": {
                "app": "hawkeye",
                "watcher_id": watcher_id,
            }
        },
        "data": {
            "log_level": "INFO",
            "watcher.json": contents,
        }
    }))
    .unwrap()
}

/// Builds an idempotent name for the `Deployment` based on the `watcher_id`.
pub fn deployment_name(watcher_id: &str) -> String {
    format!("hawkeye-deploy-{}", watcher_id)
}

/// General reference to the deployment Pod metrics server
pub fn deployment_metrics_port() -> u32 {
    3030
}

/// Builds a `Deployment` configured to run the hawkeye-worker process.
pub fn build_deployment(watcher_id: &str, ingest_port: u32) -> Deployment {
    let metric_port_str = deployment_metrics_port().to_string();
    serde_json::from_value(json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": deployment_name(watcher_id),
            "labels": {
                "app": "hawkeye",
                "watcher_id": watcher_id,
                "target_status": Status::Ready,
            }
        },
        "spec": {
            "replicas": 0,
            "selector": {
                "matchLabels": {
                    "app": "hawkeye",
                    "watcher_id": watcher_id,
                }
            },
            "template": {
                "metadata": {
                    "annotations": {
                        "prometheus.io/port": metric_port_str,
                        "prometheus.io/scrape": "true",
                        "prometheus.io/path": "metrics",
                    },
                    "labels": {
                        "app": "hawkeye",
                        "watcher_id": watcher_id,
                        "prometheus.io/port": metric_port_str,
                        "prometheus.io/scrape": "true",
                        "prometheus.io/path": "metrics",
                    }
                },
                "spec": {
                    "dnsPolicy": "Default",
                    "restartPolicy": "Always",
                    "terminationGracePeriodSeconds": 5,
                    "containers": [
                        container_spec(watcher_id, ingest_port)
                    ],
                    "volumes": [
                        {
                            "name": "config",
                            "configMap": {
                                "name": configmap_name(watcher_id),
                                "items": [
                                    {
                                        "key": "watcher.json",
                                        "path": "watcher.json"
                                    }
                                ]
                            }
                        }
                    ]
                }
            }
        }
    }))
    .unwrap()
}

/// Returns a fragment of the container specification
pub fn container_spec(watcher_id: &str, ingest_port: u32) -> serde_json::Value {
    json!({
        "name": "hawkeye-app",
        "imagePullPolicy": "IfNotPresent",
        "image": DOCKER_IMAGE.as_str(),
        "args": [
            "/config/watcher.json"
        ],
        "env": [
            {
                "name": "RUST_LOG",
                "valueFrom": {
                    "configMapKeyRef": {
                        "name": configmap_name(watcher_id),
                        "key": "log_level"
                    }
                }
            }
        ],
        "resources": {
            "limits": {
                "cpu": "2000m",
                "memory": "70Mi"
            },
            "requests": {
                "cpu": "1150m",
                "memory": "50Mi"
            }
        },
        "ports": [
            {
                "containerPort": ingest_port,
                "protocol": "UDP"
            },
            {
                "containerPort": deployment_metrics_port(),
                "protocol": "TCP"
            }
        ],
        "volumeMounts": [
            {
                "mountPath": "/config",
                "name": "config",
                "readOnly": true
            }
        ]
    })
}

/// Builds an idempotent name for the `Service` based on the `watcher_id`.
pub fn service_name(watcher_id: &str) -> String {
    format!("hawkeye-vid-svc-{}", watcher_id)
}

/// Builds a `Service` in the format expected to expose the hawkeye-worker.
pub fn build_service(watcher_id: &str, ingest_port: u32) -> Service {
    serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": {
            "name": service_name(watcher_id),
            "labels": {
                "app": "hawkeye",
                "watcher_id": watcher_id,
            },
            "annotations": {
                // "external-dns.alpha.kubernetes.io/hostname": "",
                "service.beta.kubernetes.io/aws-load-balancer-type": "nlb"
            }
        },
        "spec": {
            "type": "LoadBalancer",
            "externalTrafficPolicy": "Cluster",
            "selector": {
                "app": "hawkeye",
                "watcher_id": watcher_id,
            },
            "ports": [
                {
                    "name": "video-feed",
                    "protocol": "UDP",
                    "port": ingest_port,
                    "targetPort": ingest_port
                }
            ]
        }
    }))
    .unwrap()
}
