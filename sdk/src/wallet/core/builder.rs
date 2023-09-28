// Copyright 2021 IOTA Stiftung
// SPDX-License-Identifier: Apache-2.0

use std::sync::{
    atomic::{AtomicU32, AtomicUsize},
    Arc,
};
#[cfg(feature = "storage")]
use std::{collections::HashSet, sync::atomic::Ordering};

use futures::{future::try_join_all, FutureExt};
use serde::Serialize;
use tokio::sync::RwLock;

use super::operations::storage::SaveLoadWallet;
#[cfg(feature = "events")]
use crate::wallet::events::EventEmitter;
#[cfg(all(feature = "storage", not(feature = "rocksdb")))]
use crate::wallet::storage::adapter::memory::Memory;
use crate::{
    client::secret::DynSecretManagerConfig,
    wallet::{core::WalletInner, Account, ClientOptions, Wallet},
};
#[cfg(feature = "storage")]
use crate::{
    client::secret::SecretManagerConfig,
    wallet::{
        account::AccountDetails,
        storage::{StorageManager, StorageOptions},
    },
};

/// Builder for the wallet.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletBuilder {
    pub(crate) client_options: Option<ClientOptions>,
    pub(crate) coin_type: Option<u32>,
    #[cfg(feature = "storage")]
    pub(crate) storage_options: Option<StorageOptions>,
    #[serde(skip)]
    pub(crate) secret_manager: Option<Arc<RwLock<Box<dyn DynSecretManagerConfig>>>>,
}

impl Default for WalletBuilder {
    fn default() -> Self {
        Self {
            client_options: Default::default(),
            coin_type: Default::default(),
            #[cfg(feature = "storage")]
            storage_options: Default::default(),
            secret_manager: Default::default(),
        }
    }
}

impl WalletBuilder {
    /// Initialises a new instance of the wallet builder with the default storage adapter.
    pub fn new() -> Self {
        Self {
            secret_manager: None,
            ..Default::default()
        }
    }

    /// Set the client options for the core nodes.
    pub fn with_client_options(mut self, client_options: impl Into<Option<ClientOptions>>) -> Self {
        self.client_options = client_options.into();
        self
    }

    /// Set the coin type for the wallet. Registered coin types can be found at <https://github.com/satoshilabs/slips/blob/master/slip-0044.md>.
    pub fn with_coin_type(mut self, coin_type: impl Into<Option<u32>>) -> Self {
        self.coin_type = coin_type.into();
        self
    }

    /// Set the storage options to be used.
    #[cfg(feature = "storage")]
    #[cfg_attr(docsrs, doc(cfg(feature = "storage")))]
    pub fn with_storage_options(mut self, storage_options: impl Into<Option<StorageOptions>>) -> Self {
        self.storage_options = storage_options.into();
        self
    }

    /// Set the secret_manager to be used.
    pub fn with_secret_manager<S: 'static + DynSecretManagerConfig + Send>(
        mut self,
        secret_manager: impl Into<Option<S>>,
    ) -> Self {
        self.secret_manager = secret_manager.into().map(|sm| Arc::new(RwLock::new(Box::new(sm) as _)));
        self
    }

    /// Set the secret_manager to be used wrapped in an Arc<RwLock<>> so it can be cloned and mutated also outside of
    /// the Wallet.
    pub fn with_secret_manager_arc(
        mut self,
        secret_manager: impl Into<Option<Arc<RwLock<Box<dyn DynSecretManagerConfig>>>>>,
    ) -> Self {
        self.secret_manager = secret_manager.into();
        self
    }

    /// Set the storage path to be used.
    #[cfg(feature = "storage")]
    #[cfg_attr(docsrs, doc(cfg(feature = "storage")))]
    pub fn with_storage_path(mut self, path: &str) -> Self {
        self.storage_options = Some(StorageOptions {
            path: path.into(),
            ..Default::default()
        });
        self
    }
}

impl WalletBuilder
where
    Self: SaveLoadWallet,
{
    /// Loads the wallet from storage
    #[cfg(feature = "storage")]
    pub async fn load_storage<S: 'static + SecretManagerConfig + std::fmt::Debug>(
        mut self,
        storage_options: impl Into<StorageOptions>,
    ) -> crate::wallet::Result<Self>
    where
        crate::client::Error: From<S::Error>,
        S::Config: 'static,
    {
        log::debug!("[WalletBuilder::load]");
        let storage_options = storage_options.into();

        #[cfg(feature = "rocksdb")]
        let storage =
            crate::wallet::storage::adapter::rocksdb::RocksdbStorageAdapter::new(storage_options.path.clone())?;
        #[cfg(not(feature = "rocksdb"))]
        let storage = Memory::default();

        let storage_manager = StorageManager::new(storage, storage_options.encryption_key.clone()).await?;
        self.storage_options.replace(storage_options);

        let read_manager_builder = Self::load::<S>(&storage_manager).await?;

        if let Some(builder) = read_manager_builder {
            if let Some(client_options) = builder.client_options {
                self.client_options.replace(client_options);
            }

            if let Some(secret_manager) = builder.secret_manager {
                self.secret_manager.replace(secret_manager);
            }

            if let Some(coin_type) = builder.coin_type {
                self.coin_type.replace(coin_type);
            }
        }

        Ok(self)
    }

    /// Builds the wallet
    pub async fn finish(self) -> crate::wallet::Result<Wallet> {
        log::debug!("[WalletBuilder]");

        #[cfg(feature = "storage")]
        let storage_options = self.storage_options.clone().unwrap_or_default();

        // Check if the db exists and if not, return an error if one parameter is missing, because otherwise the db
        // would be created with an empty parameter which just leads to errors later
        #[cfg(feature = "storage")]
        if !storage_options.path.is_dir() {
            if self.client_options.is_none() {
                return Err(crate::wallet::Error::MissingParameter("client_options"));
            }
            if self.coin_type.is_none() {
                return Err(crate::wallet::Error::MissingParameter("coin_type"));
            }
            if self.secret_manager.is_none() {
                return Err(crate::wallet::Error::MissingParameter("secret_manager"));
            }
        }

        #[cfg(all(feature = "rocksdb", feature = "storage"))]
        let storage =
            crate::wallet::storage::adapter::rocksdb::RocksdbStorageAdapter::new(storage_options.path.clone())?;
        #[cfg(all(not(feature = "rocksdb"), feature = "storage"))]
        let storage = Memory::default();

        // Check if the db exists and if not, return an error if one parameter is missing, because otherwise the db
        // would be created with an empty parameter which just leads to errors later
        #[cfg(feature = "storage")]
        if !storage_options.path.is_dir() {
            if self.client_options.is_none() {
                return Err(crate::wallet::Error::MissingParameter("client_options"));
            }
            if self.coin_type.is_none() {
                return Err(crate::wallet::Error::MissingParameter("coin_type"));
            }
            if self.secret_manager.is_none() {
                return Err(crate::wallet::Error::MissingParameter("secret_manager"));
            }
        }

        #[cfg(feature = "storage")]
        let mut storage_manager = StorageManager::new(storage, storage_options.encryption_key.clone()).await?;

        let coin_type = self.coin_type.ok_or(crate::wallet::Error::MissingParameter(
            "coin_type (IOTA: 4218, Shimmer: 4219)",
        ))?;

        #[cfg(feature = "storage")]
        let mut accounts = storage_manager.get_accounts().await?;

        // Check against potential account coin type before saving the wallet data
        #[cfg(feature = "storage")]
        if let Some(account) = accounts.first() {
            if *account.coin_type() != coin_type {
                return Err(crate::wallet::Error::InvalidCoinType {
                    new_coin_type: coin_type,
                    existing_coin_type: *account.coin_type(),
                });
            }
        }

        // Store wallet data in storage
        #[cfg(feature = "storage")]
        self.save(&storage_manager).await?;

        #[cfg(feature = "events")]
        let event_emitter = tokio::sync::RwLock::new(EventEmitter::new());

        // It happened that inputs got locked, the transaction failed, but they weren't unlocked again, so we do this
        // here
        #[cfg(feature = "storage")]
        unlock_unused_inputs(&mut accounts)?;
        #[cfg(not(feature = "storage"))]
        let accounts = Vec::new();
        let wallet_inner = Arc::new(WalletInner {
            background_syncing_status: AtomicUsize::new(0),
            client: self
                .client_options
                .clone()
                .ok_or(crate::wallet::Error::MissingParameter("client_options"))?
                .finish()
                .await?,
            coin_type: AtomicU32::new(coin_type),
            secret_manager: self
                .secret_manager
                .ok_or(crate::wallet::Error::MissingParameter("secret_manager"))?,
            #[cfg(feature = "events")]
            event_emitter,
            #[cfg(feature = "storage")]
            storage_options,
            #[cfg(feature = "storage")]
            storage_manager: tokio::sync::RwLock::new(storage_manager),
        });

        let mut accounts: Vec<Account> = try_join_all(
            accounts
                .into_iter()
                .map(|a| Account::new(a, wallet_inner.clone()).boxed()),
        )
        .await?;

        for account in accounts.iter_mut() {
            // Safe to unwrap because we create the client if accounts aren't empty
            account.update_account_bech32_hrp().await?;
        }

        Ok(Wallet {
            inner: wallet_inner,
            accounts: Arc::new(RwLock::new(accounts)),
        })
    }

    #[cfg(feature = "storage")]
    pub(crate) async fn from_wallet(wallet: &Wallet) -> Self {
        Self {
            client_options: Some(wallet.client_options().await),
            coin_type: Some(wallet.coin_type.load(Ordering::Relaxed)),
            storage_options: Some(wallet.storage_options.clone()),
            secret_manager: Some(wallet.secret_manager.clone()),
        }
    }
}

// Check if any of the locked inputs is not used in a transaction and unlock them, so they get available for new
// transactions
#[cfg(feature = "storage")]
fn unlock_unused_inputs(accounts: &mut [AccountDetails]) -> crate::wallet::Result<()> {
    log::debug!("[unlock_unused_inputs]");
    for account in accounts.iter_mut() {
        let mut used_inputs = HashSet::new();
        for transaction_id in account.pending_transactions() {
            if let Some(tx) = account.transactions().get(transaction_id) {
                for input in &tx.inputs {
                    used_inputs.insert(*input.metadata.output_id());
                }
            }
        }
        account.locked_outputs.retain(|input| {
            let used = used_inputs.contains(input);
            if !used {
                log::debug!("unlocking unused input {input}");
            }
            used
        })
    }
    Ok(())
}

#[cfg(feature = "serde")]
pub(crate) mod dto {
    use serde::Deserialize;

    use super::*;
    #[cfg(feature = "storage")]
    use crate::wallet::storage::StorageOptions;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WalletBuilderDto {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub(crate) client_options: Option<ClientOptions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub(crate) coin_type: Option<u32>,
        #[cfg(feature = "storage")]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub(crate) storage_options: Option<StorageOptions>,
    }

    impl From<WalletBuilderDto> for WalletBuilder {
        fn from(value: WalletBuilderDto) -> Self {
            Self {
                client_options: value.client_options,
                coin_type: value.coin_type,
                #[cfg(feature = "storage")]
                storage_options: value.storage_options,
                secret_manager: None,
            }
        }
    }

    impl<'de> Deserialize<'de> for WalletBuilder {
        fn deserialize<D>(d: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            WalletBuilderDto::deserialize(d).map(Into::into)
        }
    }
}
