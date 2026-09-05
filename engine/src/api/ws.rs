//! WebSocket ke side panel.
//!
//! INVARIAN
//! - Browser tidak bisa memasang header Authorization pada koneksi WebSocket,
//!   dan token TIDAK BOLEH ditaruh di query string. Karena itu token dititipkan
//!   pada `Sec-WebSocket-Protocol`, yang bisa diatur lewat argumen kedua
//!   `new WebSocket(url, protocols)`.
//! - Server menjawab dengan subprotocol `rangefinder` saja, tidak pernah
//!   menggemakan token kembali.
//! - Panel adalah penerima pasif. Semua perubahan status didorong dari engine,
//!   tidak ada polling dari sisi panel. Lihat PRD 3.5.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};

use super::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use super::{
    token_from_subprotocols, AppState, ENGINE_VERSION, WS_SUBPROTOCOL, WS_UNAUTHORIZED,
};

pub async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let header = headers.get(axum::http::header::SEC_WEBSOCKET_PROTOCOL);
    if !token_from_subprotocols(header, &state.auth) {
        tracing::warn!("koneksi WebSocket ditolak, token pairing tidak cocok");
        return (WS_UNAUTHORIZED, "token pairing tidak cocok").into_response();
    }

    ws.protocols([WS_SUBPROTOCOL])
        .on_upgrade(move |socket| session(socket, state))
}

async fn session(socket: WebSocket, state: AppState) {
    let (mut tx, mut rx) = socket.split();
    let mut events = state.events.subscribe();

    // Sapaan dan status awal, supaya panel tidak perlu meminta apa apa dulu.
    let hello = ServerMessage::Hello {
        engine_version: ENGINE_VERSION.to_string(),
        protocol_version: PROTOCOL_VERSION,
    };
    if send(&mut tx, &hello).await.is_err() {
        return;
    }
    let initial = ServerMessage::EngineStatus(Box::new(state.status().await));
    if send(&mut tx, &initial).await.is_err() {
        return;
    }

    tracing::info!("panel tersambung");

    loop {
        tokio::select! {
            // Dorongan dari engine.
            event = events.recv() => {
                match event {
                    Ok(msg) => {
                        if send(&mut tx, &msg).await.is_err() {
                            break;
                        }
                    }
                    // Panel tertinggal terlalu jauh. Lebih baik kirim status
                    // terbaru daripada memutus koneksi.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(terlewat = n, "panel tertinggal, mengirim status terbaru");
                        let msg = ServerMessage::EngineStatus(Box::new(state.status().await));
                        if send(&mut tx, &msg).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // Permintaan dari panel.
            incoming = rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::GetStatus) => {
                                let msg = ServerMessage::EngineStatus(Box::new(state.status().await));
                                if send(&mut tx, &msg).await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::Ping) => {
                                // Denyut juga memundurkan kunci otomatis, karena
                                // panel yang terbuka berarti user masih bekerja.
                                state.wallet.lock().await.touch();
                                if send(&mut tx, &ServerMessage::Pong).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "pesan panel tidak dikenali, diabaikan");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // Biner dan ping bawaan diabaikan.
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "koneksi panel bermasalah");
                        break;
                    }
                }
            }
        }
    }

    tracing::info!("panel terputus");
}

async fn send<S>(tx: &mut S, msg: &ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!(error = %e, "gagal membuat JSON untuk pesan server");
            return Err(());
        }
    };
    tx.send(Message::Text(json.into())).await.map_err(|_| ())
}
