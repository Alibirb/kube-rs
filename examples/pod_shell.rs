#[macro_use]
extern crate log;

use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;

use kube::{
    api::{Api, AttachParams, AttachedProcess, DeleteParams, ListParams, Meta, PostParams, WatchEvent},
    Client,
};
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::env::set_var("RUST_LOG", "info,kube=debug");
    env_logger::init();
    let client = Client::try_default().await?;
    let namespace = std::env::var("NAMESPACE").unwrap_or_else(|_| "default".into());

    let p: Pod = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": { "name": "example" },
        "spec": {
            "containers": [{
                "name": "example",
                "image": "alpine",
                // Do nothing
                "command": ["tail", "-f", "/dev/null"],
            }],
        }
    }))?;

    let pods: Api<Pod> = Api::namespaced(client, &namespace);
    // Stop on error including a pod already exists or is still being deleted.
    pods.create(&PostParams::default(), &p).await?;

    // Wait until the pod is running, otherwise we get 500 error.
    let lp = ListParams::default().fields("metadata.name=example").timeout(10);
    let mut stream = pods.watch(&lp, "0").await?.boxed();
    while let Some(status) = stream.try_next().await? {
        match status {
            WatchEvent::Added(o) => {
                info!("Added {}", Meta::name(&o));
            }
            WatchEvent::Modified(o) => {
                let s = o.status.as_ref().expect("status exists on pod");
                if s.phase.clone().unwrap_or_default() == "Running" {
                    info!("Ready to attach to {}", Meta::name(&o));
                    break;
                }
            }
            _ => {}
        }
    }

    // Stdin example
    {
        let mut attached = pods
            .exec(
                "example",
                vec!["sh"],
                &AttachParams {
                    stdin: true,
                    stdout: true,
                    stderr: false,
                    tty: true,
                    ..AttachParams::default()
                },
            )
            .await?;
        let mut stdin_writer = attached.stdin().unwrap();
        let mut stdout_stream = attached.stdout().unwrap();
        let mut stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        // pipe current stdin to the stdin writer from ws
        tokio::spawn(async move {
            tokio::io::copy(&mut stdin, &mut stdin_writer).await;
        });
        // pipe stdout from ws to current stdout
        tokio::spawn(async move {
            // fails atm, stdout_stream not AsyncRead?
            tokio::io::copy(&mut stdout_stream, &mut stdout).await;
        });
        // wait a bit to explore
        tokio::time::delay_for(tokio::time::Duration::from_secs(15)).await;

    }

    // Delete it
    pods.delete("example", &DeleteParams::default())
        .await?
        .map_left(|pdel| {
            assert_eq!(Meta::name(&pdel), "example");
        });

    Ok(())
}
