use std::sync::Arc;

use color_eyre::Result;
use email::{
    account::config::AccountConfig,
    email::utils::{local_draft_path, remove_local_draft},
    flag::{Flag, Flags},
    folder::DRAFTS,
    template::Template,
};
use mml::MmlCompilerBuilder;
use pimalaya_tui::{
    himalaya::{
        backend::Backend,
        choice::{self, PostEditChoice, PreEditChoice},
        editor::{open_with_local_draft, open_with_tpl},
    },
    terminal::cli::printer::Printer,
};

/// Local wrapper around `pimalaya_tui::himalaya::editor::edit_tpl_with_editor`
/// that injects Apple Mail-style headers after MML compilation, before
/// sending or saving to drafts.
#[allow(unused)]
pub async fn edit_tpl_with_editor<P: Printer>(
    config: Arc<AccountConfig>,
    printer: &mut P,
    backend: &Backend,
    mut tpl: Template,
) -> Result<()> {
    let draft = local_draft_path();
    if draft.exists() {
        loop {
            match choice::pre_edit() {
                Ok(choice) => match choice {
                    PreEditChoice::Edit => {
                        tpl = open_with_local_draft().await?;
                        break;
                    }
                    PreEditChoice::Discard => {
                        tpl = open_with_tpl(tpl).await?;
                        break;
                    }
                    PreEditChoice::Quit => return Ok(()),
                },
                Err(err) => {
                    println!("{}", err);
                    continue;
                }
            }
        }
    } else {
        tpl = open_with_tpl(tpl).await?;
    }

    loop {
        match choice::post_edit() {
            Ok(PostEditChoice::Send) => {
                printer.log("Sending message…\n")?;

                #[allow(unused_mut)]
                let mut compiler = MmlCompilerBuilder::new();

                #[cfg(any(
                    feature = "pgp-gpg",
                    feature = "pgp-commands",
                    feature = "pgp-native"
                ))]
                compiler.set_some_pgp(config.pgp.clone());

                let email = compiler.build(tpl.as_str())?.compile().await?.into_vec()?;

                let email = crate::from_override::strip_excluded_recipients_in_raw_message(&email);
                let email = crate::from_override::override_from_in_raw_message(&email);
                let email = crate::from_override::inject_cc_in_raw_message(&email);
                let email = crate::from_override::encode_address_headers(&email);
                let email = crate::from_override::inject_missing_headers(&email);

                backend.send_message_then_save_copy(&email).await?;

                remove_local_draft()?;
                printer.out("Message successfully sent!\n")?;
                break;
            }
            Ok(PostEditChoice::Edit) => {
                tpl = open_with_tpl(tpl).await?;
                continue;
            }
            Ok(PostEditChoice::LocalDraft) => {
                printer.out("Message successfully saved locally!\n")?;
                break;
            }
            Ok(PostEditChoice::RemoteDraft) => {
                #[allow(unused_mut)]
                let mut compiler = MmlCompilerBuilder::new();

                #[cfg(any(
                    feature = "pgp-gpg",
                    feature = "pgp-commands",
                    feature = "pgp-native"
                ))]
                compiler.set_some_pgp(config.pgp.clone());

                let email = compiler.build(tpl.as_str())?.compile().await?.into_vec()?;

                backend
                    .add_message_with_flags(
                        DRAFTS,
                        &email,
                        &Flags::from_iter([Flag::Seen, Flag::Draft]),
                    )
                    .await?;
                remove_local_draft()?;
                printer.out("Message successfully saved to drafts!\n")?;
                break;
            }
            Ok(PostEditChoice::Discard) => {
                remove_local_draft()?;
                break;
            }
            Err(err) => {
                printer.out(format!("{err}\n"));
                continue;
            }
        }
    }

    Ok(())
}
