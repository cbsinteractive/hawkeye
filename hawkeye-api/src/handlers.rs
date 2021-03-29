use crate::config::{NAMESPACE, CALL_WATCHER_TIMEOUT};
use crate::templates;
use crate::templates::container_spec;
use hawkeye_core::models::{Status, Watcher};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Service};
use kube::api::{DeleteParams, ListParams, PatchParams, PostParams};
use kube::{Api, Client};
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use uuid::Uuid;
use warp::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use warp::http::{HeaderValue, StatusCode};
use warp::hyper::Body;
use warp::reply;

pub async fn list_watchers(client: Client) -> Result<impl warp::Reply, Infallible> {
    let lp = ListParams::default()
        .labels("app=hawkeye,watcher_id")
        .timeout(10);

    // Get all deployments we know, we want to return the status of each watcher
    let deployments_client: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    let deployments = deployments_client.list(&lp).await.unwrap();
    let mut deployments_index = HashMap::new();
    for deploy in deployments.items {
        if let Some(watcher_id) = deploy.metadata.labels.as_ref().unwrap().get("watcher_id") {
            deployments_index.insert(watcher_id.clone(), deploy.get_watcher_status());
        }
    }

    let config_maps_client: Api<ConfigMap> = Api::namespaced(client.clone(), &NAMESPACE);
    let config_maps = config_maps_client.list(&lp).await.unwrap();

    let mut watchers: Vec<Watcher> = Vec::new();
    for config in config_maps.items {
        let data = config.data.unwrap();
        let mut w: Watcher = serde_json::from_str(data.get("watcher.json").unwrap()).unwrap();
        let calculated_status = if let Some(status) =
            deployments_index.get(w.id.as_ref().unwrap_or(&"undefined".to_string()))
        {
            *status
        } else {
            Status::Error
        };
        w.status = Some(calculated_status);
        // TODO: Comes from the service
        w.source.ingest_ip = None;
        watchers.push(w);
    }

    Ok(warp::reply::json(&watchers))
}

pub async fn create_watcher(
    mut watcher: Watcher,
    client: Client,
) -> Result<impl warp::Reply, Infallible> {
    log::debug!("v1.create_watcher: {:?}", watcher);

    let new_id = Uuid::new_v4().to_string();
    watcher.id = Some(new_id.clone());
    let pp = PostParams::default();

    // 1. Create ConfigMap
    log::debug!("Creating ConfigMap instance");
    let config_maps: Api<ConfigMap> = Api::namespaced(client.clone(), &NAMESPACE);
    let config_file_contents = serde_json::to_string(&watcher).unwrap();
    let config = templates::build_configmap(&new_id, &config_file_contents);
    // TODO: Handle errors
    let _ = config_maps.create(&pp, &config).await.unwrap();

    // 2. Create Deployment with replicas=0
    log::debug!("Creating Deployment instance");
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    let deploy = templates::build_deployment(&new_id, watcher.source.ingest_port);
    // TODO: Handle errors
    let _ = deployments.create(&pp, &deploy).await.unwrap();

    // 3. Create Service/LoadBalancer
    log::debug!("Creating Service instance");
    let services: Api<Service> = Api::namespaced(client.clone(), &NAMESPACE);
    let svc = templates::build_service(&new_id, watcher.source.ingest_port);
    // TODO: Handle errors
    let _ = services.create(&pp, &svc).await.unwrap();

    watcher.status = Some(Status::Pending);
    watcher.source.ingest_ip = None;

    Ok(reply::with_status(
        reply::json(&watcher),
        StatusCode::CREATED,
    ))
}

pub async fn upgrade_watcher(id: String, client: Client) -> Result<impl warp::Reply, Infallible> {
    log::debug!("v1.upgrade_watcher: {}", id);
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    let deployment = match deployments.get(&templates::deployment_name(&id)).await {
        Ok(d) => d,
        Err(_) => {
            return Ok(reply::with_status(
                reply::json(&json!({})),
                StatusCode::NOT_FOUND,
            ))
        }
    };

    // We use the ConfigMap as source of truth for what are the watchers we have
    let config_maps_client: Api<ConfigMap> = Api::namespaced(client.clone(), &NAMESPACE);
    let config_map = match config_maps_client
        .get(&templates::configmap_name(&id))
        .await
    {
        Ok(c) => c,
        Err(_) => {
            return Ok(reply::with_status(
                reply::json(&json!({})),
                StatusCode::NOT_FOUND,
            ))
        }
    };

    let mut watcher: Watcher =
        serde_json::from_str(config_map.data.unwrap().get("watcher.json").unwrap()).unwrap();
    let watcher_status = deployment.get_watcher_status();
    if watcher_status != Status::Ready {
        return Ok(reply::with_status(
            reply::json(
                &json!({"message": "The Watcher must be stopped before the upgrade can be applied"}),
            ),
            StatusCode::BAD_REQUEST,
        ));
    }
    watcher.status = Some(watcher_status);

    let patch_params = PatchParams::default();
    let spec_updated = json!({
        "spec": {
            "template": {
                "spec": {
                    "containers": [
                        container_spec(&id, watcher.source.ingest_port)
                    ]
                }
            }
        }
    });

    match deployments
        .patch(
            deployment.metadata.name.as_ref().unwrap(),
            &patch_params,
            serde_json::to_vec(&spec_updated).unwrap(),
        )
        .await
    {
        Ok(_) => Ok(reply::with_status(reply::json(&watcher), StatusCode::OK)),
        Err(e) => {
            let msg: String = format!("Error while calling Kubernetes API: {:?}", e);
            log::error!("{}", msg);
            let error_body = json!({ "message": msg });
            return Ok(reply::with_status(
                reply::json(&error_body),
                StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
    }
}

pub async fn get_watcher(id: String, client: Client) -> Result<impl warp::Reply, Infallible> {
    let deployments_client: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    // TODO: searching for a deployment could be a filter in this route
    let deployment = match deployments_client
        .get(&templates::deployment_name(&id))
        .await
    {
        Ok(d) => d,
        Err(_) => {
            return Ok(reply::with_status(
                reply::json(&json!({})),
                StatusCode::NOT_FOUND,
            ))
        }
    };

    // We use the ConfigMap as source of truth for what are the watchers we have
    let config_maps_client: Api<ConfigMap> = Api::namespaced(client.clone(), &NAMESPACE);
    let config_map = match config_maps_client
        .get(&templates::configmap_name(&id))
        .await
    {
        Ok(c) => c,
        Err(_) => {
            return Ok(reply::with_status(
                reply::json(&json!({})),
                StatusCode::NOT_FOUND,
            ))
        }
    };

    let mut w: Watcher =
        serde_json::from_str(config_map.data.unwrap().get("watcher.json").unwrap()).unwrap();
    w.status = Some(deployment.get_watcher_status());

    w.status_description = if let Some(Status::Pending) = w.status.as_ref() {
        // Load more information why it's in pending status
        // We get the reason the container is waiting, if available
        let pods_client: Api<Pod> = Api::namespaced(client.clone(), &NAMESPACE);
        let lp = ListParams::default().labels(&format!("app=hawkeye,watcher_id={}", id));
        let pods = pods_client.list(&lp).await.unwrap();
        let status_description = pods
            .items
            .first()
            .map(|p| p.status.as_ref())
            .flatten()
            .map(|ps| ps.container_statuses.as_ref())
            .flatten()
            .map(|css| css.first())
            .flatten()
            .map(|cs| cs.state.as_ref())
            .flatten()
            .map(|cs| cs.waiting.as_ref())
            .flatten()
            .map(|csw| csw.message.clone())
            .flatten();
        log::debug!(
            "Additional information for the Pending status: {:?}",
            status_description.as_ref()
        );
        status_description
    } else {
        None
    };

    // Comes from the service
    w.source.ingest_ip = if w.status != Some(Status::Error) {
        log::debug!("Getting ingest_ip from Service's LoadBalancer");
        let services: Api<Service> = Api::namespaced(client.clone(), &NAMESPACE);
        let service = services
            .get_status(&templates::service_name(&id))
            .await
            .unwrap();
        service
            .status
            .as_ref()
            .map(|s| s.load_balancer.as_ref())
            .flatten()
            .map(|lbs| lbs.ingress.as_ref())
            .flatten()
            .map(|lbs| lbs.first())
            .flatten()
            .map(|lb| lb.clone().hostname.or(lb.clone().ip))
            .flatten()
    } else {
        None
    };

    Ok(reply::with_status(reply::json(&w), StatusCode::OK))
}

pub async fn get_video_frame(id: String, client: Client) -> Result<impl warp::Reply, Infallible> {
    let mut resp = warp::reply::Response::new(Body::empty());

    // We use the ConfigMap as source of truth for what are the watchers we have
    let config_maps_client: Api<ConfigMap> = Api::namespaced(client.clone(), &NAMESPACE);
    let config_map = match config_maps_client
        .get(&templates::configmap_name(&id))
        .await
    {
        Ok(c) => c,
        Err(_) => {
            log::debug!("ConfigMap object not found for this watcher: {}", id);
            *resp.status_mut() = StatusCode::NOT_FOUND;
            return Ok(resp);
        }
    };
    let watcher: Watcher =
        serde_json::from_str(config_map.data.unwrap().get("watcher.json").unwrap()).unwrap();

    let deployments_client: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    let deployment = match deployments_client
        .get(&templates::deployment_name(&id))
        .await
    {
        Ok(d) => d,
        Err(_) => {
            *resp.status_mut() = StatusCode::NOT_FOUND;
            return Ok(resp);
        }
    };
    if Status::Running != deployment.get_watcher_status() {
        log::debug!("Watcher is not running..");
        *resp.status_mut() = StatusCode::NOT_ACCEPTABLE;
        return Ok(resp);
    }
    let pods_client: Api<Pod> = Api::namespaced(client.clone(), &NAMESPACE);
    let lp = ListParams::default().labels(&format!("app=hawkeye,watcher_id={}", id));
    let pods = pods_client.list(&lp).await.unwrap();
    if let Some(pod_ip) = pods
        .items
        .first()
        .map(|p| p.status.as_ref())
        .flatten()
        .map(|ps| ps.pod_ip.clone())
        .flatten()
    {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(*CALL_WATCHER_TIMEOUT))
            .build()
            .unwrap();
        // Try for new and old ports in pod
        for port in vec![watcher.source.ingest_port, 3030] {
            let url = format!("http://{}:{}/latest_frame", pod_ip, port);

            log::info!("Calling Pod using url: {}", url);
            let response=  match http_client
                .get(url.as_str())
                .send()
                .await {
                Ok(r) => r,
                Err(error) => {
                    log::error!("Could not call {} endpoint: {:?}", url, error);
                    *resp.status_mut() = StatusCode::EXPECTATION_FAILED;
                    return Ok(resp);
                }
            };

            match response
                .error_for_status()
            {
                Ok(image_response) => {
                    let image_bytes = image_response.bytes().await.unwrap();
                    let headers = resp.headers_mut();
                    headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
                    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
                    *resp.body_mut() = Body::from(image_bytes);
                    return Ok(resp);
                }
                Err(_) => {
                    continue;
                }
            }
        }
        log::error!("Error calling Pod using old and new urls");
        *resp.status_mut() = StatusCode::EXPECTATION_FAILED;
    } else {
        log::debug!("Not able to get Pod IP");
        *resp.status_mut() = StatusCode::EXPECTATION_FAILED;
    }
    Ok(resp)
}

pub async fn start_watcher(id: String, client: Client) -> Result<impl warp::Reply, Infallible> {
    let deployments_client: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    // TODO: probably better to just get the scale
    let deployment = match deployments_client
        .get(&templates::deployment_name(&id))
        .await
    {
        Ok(d) => d,
        Err(_) => {
            return Ok(reply::with_status(
                reply::json(&json!({})),
                StatusCode::NOT_FOUND,
            ))
        }
    };
    match deployment.get_watcher_status() {
        Status::Running => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher is already running"
            })),
            StatusCode::OK,
        )),
        Status::Pending => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher is updating"
            })),
            StatusCode::CONFLICT,
        )),
        Status::Ready => {
            // Start watcher / replicas to 1
            let patch_params = PatchParams::default();

            let fs = json!({
                "spec": { "replicas": 1 }
            });
            let o = deployments_client
                .patch_scale(
                    deployment.metadata.name.as_ref().unwrap(),
                    &patch_params,
                    serde_json::to_vec(&fs).unwrap(),
                )
                .await
                .unwrap();
            log::debug!("Scale status: {:?}", o);

            let status_label = json!({
                "metadata": {
                    "labels": {
                        "target_status": Status::Running
                    }
                }
            });
            let _ = deployments_client
                .patch(
                    deployment.metadata.name.as_ref().unwrap(),
                    &patch_params,
                    serde_json::to_vec(&status_label).unwrap(),
                )
                .await;

            Ok(reply::with_status(
                reply::json(&json!({
                    "message": "Watcher is starting"
                })),
                StatusCode::OK,
            ))
        }
        Status::Error => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher in error state cannot be set to running"
            })),
            StatusCode::NOT_ACCEPTABLE,
        )),
    }
}

pub async fn stop_watcher(id: String, client: Client) -> Result<impl warp::Reply, Infallible> {
    let deployments_client: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    // TODO: probably better to just get the scale
    let deployment = match deployments_client
        .get(&templates::deployment_name(&id))
        .await
    {
        Ok(d) => d,
        Err(_) => {
            return Ok(reply::with_status(
                reply::json(&json!({})),
                StatusCode::NOT_FOUND,
            ))
        }
    };
    // TODO: Set target_status to Ready
    match deployment.get_watcher_status() {
        Status::Ready => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher is already stopped"
            })),
            StatusCode::OK,
        )),
        Status::Pending => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher is updating"
            })),
            StatusCode::CONFLICT,
        )),
        Status::Running => {
            // Stop watcher / replicas to 0
            let patch_params = PatchParams::default();

            let fs = json!({
                "spec": { "replicas": 0 }
            });
            let o = deployments_client
                .patch_scale(
                    deployment.metadata.name.as_ref().unwrap(),
                    &patch_params,
                    serde_json::to_vec(&fs).unwrap(),
                )
                .await
                .unwrap();
            log::debug!("Scale status: {:?}", o);

            let status_label = json!({
                "metadata": {
                    "labels": {
                        "target_status": Status::Ready
                    }
                }
            });
            let _ = deployments_client
                .patch(
                    deployment.metadata.name.as_ref().unwrap(),
                    &patch_params,
                    serde_json::to_vec(&status_label).unwrap(),
                )
                .await;

            Ok(reply::with_status(
                reply::json(&json!({
                    "message": "Watcher is stopping"
                })),
                StatusCode::OK,
            ))
        }
        Status::Error => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher in error state cannot be set to stopped"
            })),
            StatusCode::NOT_ACCEPTABLE,
        )),
    }
}

pub async fn delete_watcher(id: String, client: Client) -> Result<impl warp::Reply, Infallible> {
    let dp = DeleteParams::default();

    let deployments_client: Api<Deployment> = Api::namespaced(client.clone(), &NAMESPACE);
    let _ = deployments_client
        .delete(&templates::deployment_name(&id), &dp)
        .await;

    let config_maps: Api<ConfigMap> = Api::namespaced(client.clone(), &NAMESPACE);
    let _ = config_maps
        .delete(&templates::configmap_name(&id), &dp)
        .await;

    let services: Api<Service> = Api::namespaced(client, &NAMESPACE);
    match services.delete(&templates::service_name(&id), &dp).await {
        Ok(_) => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher has been deleted"
            })),
            StatusCode::OK,
        )),
        Err(_) => Ok(reply::with_status(
            reply::json(&json!({
                "message": "Watcher does not exist"
            })),
            StatusCode::NOT_FOUND,
        )),
    }
}

pub async fn healthcheck(client: Client) -> Result<impl warp::Reply, Infallible> {
    match client.apiserver_version().await {
        Ok(_info) => Ok(reply::with_status(
            reply::json(&json!({
                "message": "All good! 🎉",
            })),
            StatusCode::OK,
        )),
        Err(err) => {
            log::error!("Cannot communicate with K8s API: {:?}", err);
            Ok(reply::with_status(
                reply::json(&json!({
                    "message": "Not able to communicate with the Kubernetes API Server.",
                })),
                StatusCode::SERVICE_UNAVAILABLE,
            ))
        }
    }
}

trait WatcherStatus {
    fn get_watcher_status(&self) -> Status;
}

impl WatcherStatus for Deployment {
    fn get_watcher_status(&self) -> Status {
        let target_status = self
            .metadata
            .labels
            .as_ref()
            .map(|labels| {
                labels
                    .get("target_status")
                    .map(|status| serde_json::from_str(&format!("\"{}\"", status)).ok())
            })
            .flatten()
            .flatten()
            .unwrap_or({
                let name = self.metadata.name.as_ref().expect("Name must be present");
                log::error!(
                    "Deployment {} is missing required 'target_status' label",
                    name
                );
                Status::Error
            });
        if let Some(status) = self.status.as_ref() {
            let deploy_status = if status.available_replicas.unwrap_or(0) > 0 {
                Status::Running
            } else {
                Status::Ready
            };
            match (deploy_status, target_status) {
                (Status::Running, Status::Running) => Status::Running,
                (Status::Ready, Status::Ready) => Status::Ready,
                (Status::Ready, Status::Running) => Status::Pending,
                (Status::Running, Status::Ready) => Status::Pending,
                (_, _) => Status::Error,
            }
        } else {
            Status::Error
        }
    }
}
