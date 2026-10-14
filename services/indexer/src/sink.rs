//! NATS JetStream publisher. The `Nats-Msg-Id` header makes JetStream drop duplicate publishes
//! made within its window, so a replay after a crash does not double-count (RS-04).

use async_nats::jetstream::{self, stream};
use async_trait::async_trait;
use vigil_common::messages::{
    BlockMessage, EventMessage, MessageKind, STREAM_NAME, SUBJECT_BLOCKS, SUBJECT_EVENTS,
};

use crate::engine::EventSink;

pub struct NatsSink {
    js: jetstream::Context,
}

impl NatsSink {
    /// Creates the stream if it does not exist yet.
    pub async fn connect(client: async_nats::Client) -> anyhow::Result<Self> {
        let js = jetstream::new(client);
        js.get_or_create_stream(stream::Config {
            name: STREAM_NAME.to_string(),
            subjects: vec!["vigil.>".to_string()],
            ..Default::default()
        })
        .await?;
        Ok(Self { js })
    }
}

#[async_trait]
impl EventSink for NatsSink {
    async fn publish_event(&self, msg: &EventMessage) -> anyhow::Result<()> {
        let kind = match msg.kind {
            MessageKind::Log => "log",
            MessageKind::Retract => "retract",
        };
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(
            "Nats-Msg-Id",
            format!("{kind}:{:#x}:{}", msg.block_hash, msg.log_index).as_str(),
        );
        headers.insert("trace-id", msg.trace_id.as_str());
        let payload = serde_json::to_vec(msg)?;
        self.js
            .publish_with_headers(SUBJECT_EVENTS, headers, payload.into())
            .await?
            .await?;
        Ok(())
    }

    async fn publish_block(&self, msg: &BlockMessage) -> anyhow::Result<()> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(
            "Nats-Msg-Id",
            format!("block:{:#x}", msg.block_hash).as_str(),
        );
        headers.insert("trace-id", msg.trace_id.as_str());
        let payload = serde_json::to_vec(msg)?;
        self.js
            .publish_with_headers(SUBJECT_BLOCKS, headers, payload.into())
            .await?
            .await?;
        Ok(())
    }
}
