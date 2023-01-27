use std::{net::SocketAddr, sync::Arc};

use futures::FutureExt;
use jsonrpsee::server::{RandomStringIdProvider, RpcModule, ServerBuilder};
use tokio::task::JoinHandle;

use crate::{
    client::Client,
    config::Config,
    middleware::{
        call::{self, CallRequest},
        subscription::{self, SubscriptionRequest},
        Middlewares,
    },
};

// TODO: https://github.com/paritytech/jsonrpsee/issues/985
fn string_to_static_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

pub async fn start_server(
    config: &Config,
    client: Client,
) -> anyhow::Result<(SocketAddr, JoinHandle<()>)> {
    let service_builder = tower::ServiceBuilder::new();

    let server = ServerBuilder::default()
        .set_middleware(service_builder)
        .max_connections(config.server.max_connections)
        .set_id_provider(RandomStringIdProvider::new(16))
        .build((config.server.listen_address.as_str(), config.server.port))
        .await?;

    let mut module = RpcModule::new(());

    let client = Arc::new(client);

    let upstream = Arc::new(call::UpstreamMiddleware::new(client.clone()));

    for method in &config.rpcs.methods {
        let middlewares = Arc::new(Middlewares::new(
            vec![upstream.clone()],
            Arc::new(|_| {
                async {
                    Err(
                        jsonrpsee::types::error::CallError::Failed(anyhow::Error::msg(
                            "Bad configuration",
                        ))
                        .into(),
                    )
                }
                .boxed()
            }),
        ));

        let method_name = string_to_static_str(method.method.clone());
        module.register_async_method(method_name, move |params, _| {
            let middlewares = middlewares.clone();
            async move {
                middlewares
                    .call(CallRequest {
                        method: method_name.into(),
                        params: params.into_owned(),
                    })
                    .await
            }
        })?;
    }

    let rpc_methods = config
        .rpcs
        .methods
        .iter()
        .map(|m| m.method.clone())
        .chain(
            config
                .rpcs
                .subscriptions
                .iter()
                .map(|s| s.subscribe.clone()),
        )
        .chain(
            config
                .rpcs
                .subscriptions
                .iter()
                .map(|s| s.unsubscribe.clone()),
        )
        .chain(config.rpcs.aliases.iter().map(|(_, new)| new.clone()))
        .collect::<Vec<_>>();

    module.register_method("rpc_methods", move |_, _| {
        #[derive(serde::Serialize)]
        struct RpcMethodsResp {
            methods: Vec<String>,
        }

        Ok(RpcMethodsResp {
            methods: rpc_methods.clone(),
        })
    })?;

    let upstream = Arc::new(subscription::UpstreamMiddleware::new(client.clone()));

    for subscription in &config.rpcs.subscriptions {
        let subscribe_name = string_to_static_str(subscription.subscribe.clone());
        let unsubscribe_name = string_to_static_str(subscription.unsubscribe.clone());
        let name = string_to_static_str(subscription.name.clone());

        let middlewares = Arc::new(Middlewares::new(
            vec![upstream.clone()],
            Arc::new(|_| {
                async {
                    Err(
                        jsonrpsee::types::error::CallError::Failed(anyhow::Error::msg(
                            "Bad configuration",
                        ))
                        .into(),
                    )
                }
                .boxed()
            }),
        ));

        module.register_subscription(
            subscribe_name,
            name,
            unsubscribe_name,
            move |params, sink, _| {
                let params = params.into_owned();

                let middlewares = middlewares.clone();

                tokio::spawn(async move {
                    let res = middlewares
                        .call(SubscriptionRequest {
                            subscribe: subscribe_name.into(),
                            params: params.clone(),
                            unsubscribe: unsubscribe_name.into(),
                            sink,
                        })
                        .await;
                    if let Err(e) = res {
                        log::error!("Error while handling subscription: {}", e);
                    }
                });
                Ok(())
            },
        )?;
    }

    for (alias_old, alias_new) in &config.rpcs.aliases {
        let alias_old = string_to_static_str(alias_old.clone());
        let alias_new = string_to_static_str(alias_new.clone());
        module.register_alias(alias_new, alias_old)?;
    }

    let addr = server.local_addr()?;
    let handle = server.start(module)?;

    let handle = tokio::spawn(handle.stopped());

    Ok((addr, handle))
}
