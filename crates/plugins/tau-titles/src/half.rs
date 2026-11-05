//! tau-titles' host half: a title is one request to the person's model,
//! booked to the plugin.

use std::sync::Arc;

use anyhow::Context as _;
use tau_agent::plugin::Plugin;
use tau_ai::message::{Message, UserContent, UserMessage};
use tau_ui_plugin::{
    AskSettings,
    Group,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    RunCtx,
    slots::Fills,
    title::{TITLE, Title, Written},
};
use tokio_util::sync::CancellationToken;

use crate::{NAME, Settings, TitlesUi, format, model, parse, request};

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct TitlesHost;

/// What the host half keeps: the host's context, for the person's model
/// and settings as each title is written.
pub struct Host {
    cx: HostCx,
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self { cx: cx.clone() })
    }
}

impl HostHalf for TitlesHost {
    type Plugin = TitlesUi;
    type Host = Host;

    async fn agent_plugins(
        &self,
        _: &Host,
        _: &RunCtx,
        _: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _: &Host,
        _: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        PluginInfo {
            description: if settings.on {
                "Titles each chat with a short request to the plan's \
                 smallest model"
            } else {
                "Titles chats: off, each keeps its prompt's first line"
            }
            .into(),
            group: Group::App,
            ..Default::default()
        }
    }

    fn fill(&self, host: &Host, fills: &mut Fills) {
        fills.fill(
            &TITLE,
            Arc::new(Titler {
                cx: host.cx.clone(),
            }) as Arc<dyn Title>,
        );
    }
}

/// Writes titles with the person's model.
struct Titler {
    cx: HostCx,
}

impl Title for Titler {
    fn write<'a>(&'a self, task: &'a str, run_model: &'a str) -> Written<'a> {
        Box::pin(async move {
            if !self.cx.settings::<Settings>(NAME).on {
                return Ok(None);
            }
            let Some(asked) = self.cx.model(NAME) else {
                return Ok(None);
            };
            let (model, reasoning) = model(run_model);
            let settings =
                AskSettings::new(model, reasoning).text_format(format());
            let input = [Message::User(UserMessage {
                content: UserContent::Text(request(task)),
                timestamp: 0,
            })];
            let reply = asked
                .ask(&settings, &input, &[], &CancellationToken::new())
                .await?;
            if let Some(error) = reply.error_message {
                anyhow::bail!(error);
            }
            let text = reply.text();
            parse(&text)
                .map(Some)
                .with_context(|| format!("not a title: {text:?}"))
        })
    }
}
