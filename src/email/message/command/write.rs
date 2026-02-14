use std::sync::Arc;

use clap::Parser;
use color_eyre::Result;
use email::{
    config::Config,
    {backend::feature::BackendFeatureSource, message::Message},
};
use pimalaya_tui::{
    himalaya::backend::BackendBuilder,
    terminal::{cli::printer::Printer, config::TomlConfig as _},
};
use tracing::info;

use crate::{
    account::arg::name::AccountNameFlag,
    config::TomlConfig,
    message::arg::{body::MessageRawBodyArg, header::HeaderRawArgs},
};

/// Compose a new message, from scratch.
///
/// This command allows you to write a new message using the editor
/// defined in your environment variable $EDITOR. When the edition
/// process finishes, you can choose between saving or sending the
/// final message.
#[derive(Debug, Parser)]
pub struct MessageWriteCommand {
    #[command(flatten)]
    pub headers: HeaderRawArgs,

    #[command(flatten)]
    pub body: MessageRawBodyArg,

    #[command(flatten)]
    pub account: AccountNameFlag,
}

impl MessageWriteCommand {
    pub async fn execute(self, printer: &mut impl Printer, config: &TomlConfig) -> Result<()> {
        info!("executing write message command");

        let (toml_account_config, account_config) = config
            .clone()
            .into_account_configs(self.account.name.as_deref(), |c: &Config, name| {
                c.account(name).ok()
            })?;

        let mut account_config = account_config;
        crate::from_override::apply_from_override(&mut account_config);
        let account_config = Arc::new(account_config);

        let backend = BackendBuilder::new(
            Arc::new(toml_account_config),
            account_config.clone(),
            |builder| {
                builder
                    .without_features()
                    .with_add_message(BackendFeatureSource::Context)
                    .with_send_message(BackendFeatureSource::Context)
            },
        )
        .build()
        .await?;

        let mut tpl = Message::new_tpl_builder(account_config.clone())
            .with_headers(self.headers.raw)
            .with_body(self.body.raw())
            .build()
            .await?;

        crate::from_override::strip_excluded_recipients_in_tpl(&mut tpl.content);
        crate::from_override::inject_cc_in_tpl(&mut tpl.content);

        crate::editor_wrapper::edit_tpl_with_editor(account_config, printer, &backend, tpl).await
    }
}
