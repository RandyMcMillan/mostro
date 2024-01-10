use crate::util::{get_fee, get_market_quote, send_dm, show_hold_invoice};

use anyhow::Result;
use mostro_core::message::{Action, Content, Message};
use mostro_core::order::{Order, Status};
use nostr_sdk::prelude::*;
use sqlx::{Pool, Sqlite};
use sqlx_crud::Crud;
use std::str::FromStr;
use std::thread;
use tracing::error;

pub async fn take_buy_action(
    msg: Message,
    event: &Event,
    my_keys: &Keys,
    client: &Client,
    pool: &Pool<Sqlite>,
) -> Result<()> {
    // Safe unwrap as we verified the message
    let order_id = msg.get_inner_message_kind().id.unwrap();
    let mut order = match Order::by_id(pool, order_id).await? {
        Some(order) => order,
        None => {
            error!("Order Id {order_id} not found!");
            return Ok(());
        }
    };
    // We check if the message have a pubkey
    if msg.get_inner_message_kind().pubkey.is_none() {
        // We create a Message
        let message = Message::cant_do(Some(order.id), None, None);
        send_dm(client, my_keys, &event.pubkey, message.as_json()?).await?;

        return Ok(());
    }

    if order.kind != "Buy" {
        error!("Order Id {order_id} wrong kind");
        let message = Message::cant_do(Some(order.id), None, None);
        send_dm(client, my_keys, &event.pubkey, message.as_json()?).await?;

        return Ok(());
    }

    let order_status = match Status::from_str(&order.status) {
        Ok(s) => s,
        Err(e) => {
            error!("Order Id {order_id} wrong status: {e:?}");
            return Ok(());
        }
    };
    let buyer_pubkey = match order.buyer_pubkey.as_ref() {
        Some(pk) => XOnlyPublicKey::from_str(pk)?,
        None => {
            error!("Buyer pubkey not found for order {}!", order.id);
            return Ok(());
        }
    };
    if buyer_pubkey == event.pubkey {
        let message = Message::cant_do(Some(order.id), None, None);
        send_dm(client, my_keys, &event.pubkey, message.as_json()?).await?;

        return Ok(());
    }
    // We update the master pubkey
    order.master_seller_pubkey = msg.get_inner_message_kind().pubkey.clone();

    let seller_pubkey = event.pubkey;
    // Seller can take pending orders only
    if order_status != Status::Pending {
        // We create a Message
        let message = Message::new_order(
            Some(order.id),
            None,
            Action::FiatSent,
            Some(Content::TextMessage(format!(
                "Order Id {order_id} was already taken!"
            ))),
        );
        send_dm(client, my_keys, &seller_pubkey, message.as_json()?).await?;

        return Ok(());
    }

    // Check market price value in sats - if order was with market price then calculate
    if order.amount == 0 {
        order.amount =
            match get_market_quote(&order.fiat_amount, &order.fiat_code, order.premium).await {
                Ok(amount) => amount,
                Err(e) => {
                    error!("{:?}", e);
                    return Ok(());
                }
            };
        order.fee = get_fee(order.amount);
    }

    // Timestamp order take time
    order.taken_at = Timestamp::now().as_i64();
    let order_id = order.id;
    order.update(pool).await?;
    // We need to wait a second to be sure that the order is in the db
    thread::sleep(std::time::Duration::from_secs(1));

    show_hold_invoice(
        pool,
        client,
        my_keys,
        None,
        &buyer_pubkey,
        &seller_pubkey,
        order_id,
    )
    .await?;
    Ok(())
}
