// Copyright 2020-2022 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

use iota_sdk::wallet::{account::types::AccountIdentifier, Wallet};

use crate::{
    command::wallet::{
        accounts_command, add_account, backup_command, change_password_command, init_command,
        migrate_stronghold_snapshot_v2_to_v3_command, mnemonic_command, new_account_command, node_info_command,
        restore_command, set_node_url_command, set_pow_command, sync_command, unlock_wallet, InitParameters, WalletCli,
        WalletCommand,
    },
    error::Error,
    helper::{get_account_alias, get_decision, get_password, pick_account},
    println_log_error, println_log_info,
};

pub async fn new_wallet(cli: WalletCli) -> Result<(Option<Wallet>, Option<AccountIdentifier>), Error> {
    let storage_path = Path::new(&cli.wallet_db_path);
    let snapshot_path = Path::new(&cli.stronghold_snapshot_path);

    let (wallet, account_id) = if let Some(command) = cli.command {
        match command {
            WalletCommand::Accounts => {
                accounts_command(storage_path, snapshot_path).await?;
                return Ok((None, None));
            }
            WalletCommand::Init(init_parameters) => {
                let wallet = init_command(storage_path, snapshot_path, init_parameters).await?;
                (Some(wallet), None)
            }
            WalletCommand::Restore { backup_path } => {
                let wallet = restore_command(storage_path, snapshot_path, std::path::Path::new(&backup_path)).await?;
                (Some(wallet), None)
            }
            WalletCommand::Backup { backup_path } => {
                backup_command(storage_path, snapshot_path, std::path::Path::new(&backup_path)).await?;
                return Ok((None, None));
            }
            WalletCommand::ChangePassword => {
                let wallet = change_password_command(storage_path, snapshot_path).await?;
                (Some(wallet), None)
            }
            WalletCommand::MigrateStrongholdSnapshotV2ToV3 { path } => {
                migrate_stronghold_snapshot_v2_to_v3_command(path).await?;
                return Ok((None, None));
            }
            WalletCommand::NewAccount { alias } => {
                let (wallet, account) = new_account_command(storage_path, snapshot_path, alias).await?;
                (Some(wallet), Some(account))
            }
            WalletCommand::SetNodeUrl { url } => {
                let wallet = set_node_url_command(storage_path, snapshot_path, url).await?;
                (Some(wallet), None)
            }
            WalletCommand::SetPow {
                local_pow,
                worker_count,
            } => {
                let wallet = set_pow_command(storage_path, snapshot_path, local_pow, worker_count).await?;
                (Some(wallet), None)
            }
            WalletCommand::Sync => {
                let wallet = sync_command(storage_path, snapshot_path).await?;
                (Some(wallet), None)
            }
            WalletCommand::Mnemonic {
                output_file_name,
                output_stdout,
            } => {
                mnemonic_command(output_file_name, output_stdout).await?;
                return Ok((None, None));
            }
            WalletCommand::NodeInfo => {
                node_info_command(storage_path).await?;
                return Ok((None, None));
            }
        }
    } else {
        // no command provided, i.e. `> ./wallet`
        match (storage_path.exists(), snapshot_path.exists()) {
            (true, true) => {
                let password = get_password("Stronghold password", false)?;
                let wallet = unlock_wallet(storage_path, snapshot_path, password).await?;
                if wallet.get_accounts().await?.is_empty() {
                    create_initial_account(wallet).await?
                } else if let Some(alias) = cli.account {
                    (Some(wallet), Some(alias))
                } else if let Some(account) = pick_account(&wallet).await? {
                    (Some(wallet), Some(account.alias().await.into()))
                } else {
                    (Some(wallet), None)
                }
            }
            (false, false) => {
                if get_decision("Create a new wallet with default parameters?")? {
                    let wallet = init_command(storage_path, snapshot_path, InitParameters::default()).await?;
                    println_log_info!("Created new wallet.");
                    create_initial_account(wallet).await?
                } else {
                    WalletCli::print_help()?;
                    (None, None)
                }
            }
            (true, false) => {
                println_log_error!("Stronghold snapshot not found at '{}'.", snapshot_path.display());
                (None, None)
            }
            (false, true) => {
                println_log_error!("Wallet database not found at '{}'.", storage_path.display());
                (None, None)
            }
        }
    };
    Ok((wallet, account_id))
}

async fn create_initial_account(wallet: Wallet) -> Result<(Option<Wallet>, Option<AccountIdentifier>), Error> {
    // Ask the user whether an initial account should be created.
    if get_decision("Create initial account?")? {
        let alias = get_account_alias("New account alias", &wallet).await?;
        let account_id = add_account(&wallet, Some(alias)).await?;
        println_log_info!("Created initial account.\nType `help` to see all available account commands.");
        Ok((Some(wallet), Some(account_id)))
    } else {
        Ok((Some(wallet), None))
    }
}
