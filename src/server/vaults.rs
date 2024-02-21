use anyhow::{anyhow, bail};
use askama::Template;
use axum::Form;
use bitcoin::{
    absolute::LockTime,
    address::{NetworkChecked, NetworkUnchecked},
    transaction::Version,
    Address, Amount, Network, Sequence, Txid,
};
use serde::Deserialize;
use serde_with::{serde_as, DisplayFromStr};

use crate::{
    ctv::{
        self,
        segwit::{self, locking_address, locking_script},
        Ctv, Output,
    },
    error::AppError,
    vault::Vault,
};

#[derive(Template)]
#[template(path = "vaults/index.html.jinja")]
pub(crate) struct IndexTemplate;

pub(crate) async fn index() -> IndexTemplate {
    IndexTemplate
}

#[derive(Template)]
#[template(path = "vaults/locking.html.jinja")]
pub(crate) struct LockingTemplate {
    vault: String,
    address: Address<NetworkChecked>,
}

#[serde_as]
#[derive(Deserialize)]
pub(crate) struct LockingRequest {
    #[serde_as(as = "DisplayFromStr")]
    amount: Amount,
    cold_address: Address<NetworkUnchecked>,
    hot_address: Address<NetworkUnchecked>,
    block_delay: u16,
    network: Network,
}

impl From<LockingRequest> for Vault {
    fn from(value: LockingRequest) -> Self {
        Vault {
            hot: value.hot_address,
            cold: value.cold_address,
            amount: value.amount,
            network: value.network,
            delay: value.block_delay,
        }
    }
}

pub(crate) async fn locking(
    Form(request): Form<LockingRequest>,
) -> anyhow::Result<LockingTemplate, AppError> {
    let vault: Vault = request.into();
    let address = vault.vault_address()?.require_network(vault.network)?;
    let vault = serde_json::to_string(&vault)?;
    Ok(LockingTemplate { vault, address })
}

#[derive(Deserialize)]
pub(crate) struct UnvaultingRequest {
    vault: String,
    txid: Txid,
    vout: u32,
}

#[derive(Template)]
#[template(path = "vaults/unvaulting.html.jinja")]
pub(crate) struct UnvaultingTemplate {
    vault: String,
    tx: String,
}

pub(crate) async fn unvaulting(
    Form(request): Form<UnvaultingRequest>,
) -> anyhow::Result<UnvaultingTemplate, AppError> {
    let vault: Vault = serde_json::from_str(&request.vault)?;
    let vault_ctv = vault.vault_ctv()?;
    let spending_tx = vault_ctv.spending_tx(request.txid, request.vout)?[0].clone();
    let tx = hex::encode(bitcoin::consensus::serialize(&spending_tx));
    let vault = serde_json::to_string(&vault)?;
    Ok(UnvaultingTemplate { vault, tx })
}

#[derive(Deserialize)]
pub(crate) struct SpendingRequest {
    unvault_ctv: String,
    hot_ctv: String,
    cold_ctv: String,
    txid: Txid,
    vout: u32,
}

#[derive(Template)]
#[template(path = "vaults/spending.html.jinja")]
pub(crate) struct SpendingTemplate {
    cold_tx: String,
    hot_tx: String,
}

pub(crate) async fn spending(
    Form(request): Form<SpendingRequest>,
) -> anyhow::Result<SpendingTemplate, AppError> {
    let hot_ctv: Ctv = serde_json::from_str(&request.hot_ctv)?;
    let hot_tx = hot_ctv.spending_tx(request.txid, request.vout)?;
    let cold_ctv: Ctv = serde_json::from_str(&request.cold_ctv)?;
    let cold_tx = cold_ctv.spending_tx(request.txid, request.vout)?;
    Ok(SpendingTemplate {
        cold_tx: hex::encode(bitcoin::consensus::serialize(&cold_tx)),
        hot_tx: hex::encode(bitcoin::consensus::serialize(&hot_tx)),
    })
}
