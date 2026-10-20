//! One client connection: the appliance side of the handshake, then steady state.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::{SinkExt as _, StreamExt as _};
use openssl::ssl::SslContext;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::{Instant, interval_at};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HttpRequest, Response,
};
use tokio_tungstenite::tungstenite::http::StatusCode;

use crate::protocol::{self, DeviceInfo, Request};
use crate::state::{Change, value_item};
use crate::{SharedAppliance, SimError};

/// The WebSocket path every Home Connect appliance serves.
const WS_PATH: &str = "/homeconnect";

/// What a connection needs from the simulator.
#[derive(Debug, Clone)]
pub struct Context {
    /// The TLS server context.
    pub tls: Arc<SslContext>,
    /// The shared appliance state.
    pub appliance: SharedAppliance,
    /// Synthetic device facts for the info resources.
    pub info: Arc<DeviceInfo>,
    /// The live-update period.
    pub tick: Duration,
}

/// Serves one accepted TCP connection until the client goes away.
pub async fn serve(tcp: TcpStream, peer: SocketAddr, context: Context) {
    tracing::info!(%peer, "connection accepted");
    match serve_inner(tcp, &context).await {
        Ok(()) => tracing::info!(%peer, "connection closed"),
        Err(error) => tracing::warn!(%peer, %error, "connection ended"),
    }
}

async fn serve_inner(tcp: TcpStream, context: &Context) -> Result<(), SimError> {
    let tls = crate::tls::accept(&context.tls, tcp).await?;
    let ws = tokio_tungstenite::accept_hdr_async(tls, check_path)
        .await
        .map_err(|error| SimError::WebSocket(error.to_string()))?;
    let (mut sink, mut stream) = ws.split();
    let mut peer = Peer::new(Arc::clone(&context.appliance), Arc::clone(&context.info));
    send(&mut sink, peer.opening()).await?;

    let mut ticker = interval_at(Instant::now() + context.tick, context.tick);
    loop {
        let outgoing = tokio::select! {
            _ = ticker.tick() => peer.tick(),
            next = stream.next() => match next {
                None | Some(Ok(Message::Close(_))) => return Ok(()),
                Some(Err(error)) => return Err(SimError::WebSocket(error.to_string())),
                Some(Ok(Message::Ping(payload))) => {
                    sink.send(Message::Pong(payload))
                        .await
                        .map_err(|error| SimError::WebSocket(error.to_string()))?;
                    Vec::new()
                }
                Some(Ok(Message::Text(text))) => peer.handle(text.as_str()),
                Some(Ok(_)) => Vec::new(),
            },
        };
        for frame in outgoing {
            send(&mut sink, frame).await?;
        }
    }
}

async fn send<S>(sink: &mut S, frame: Value) -> Result<(), SimError>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    sink.send(Message::text(frame.to_string()))
        .await
        .map_err(|error| SimError::WebSocket(error.to_string()))
}

#[expect(
    clippy::result_large_err,
    reason = "the signature is fixed by tungstenite's handshake callback"
)]
fn check_path(request: &HttpRequest, response: Response) -> Result<Response, ErrorResponse> {
    if request.uri().path() == WS_PATH {
        return Ok(response);
    }
    tracing::warn!(
        path = request.uri().path(),
        "WebSocket upgrade on an unknown path refused"
    );
    let mut refusal = ErrorResponse::new(Some("not found".to_owned()));
    *refusal.status_mut() = StatusCode::NOT_FOUND;
    Err(refusal)
}

/// The protocol state of one connection, free of I/O so it can be tested directly.
#[derive(Debug)]
pub struct Peer {
    appliance: SharedAppliance,
    info: Arc<DeviceInfo>,
    s_id: u64,
    next_msg_id: u64,
    ready: bool,
}

impl Peer {
    /// A fresh connection with a random session id and message counter.
    #[must_use]
    pub fn new(appliance: SharedAppliance, info: Arc<DeviceInfo>) -> Self {
        let (s_id, next_msg_id) = {
            let mut state = appliance.lock();
            (
                u64::from(state.random_u32()),
                u64::from(state.random_u32() >> 1),
            )
        };
        Self {
            appliance,
            info,
            s_id,
            next_msg_id,
            ready: false,
        }
    }

    /// The opening `POST /ei/initialValues`.
    pub fn opening(&mut self) -> Value {
        let ed_msg_id = u64::from(self.appliance.lock().random_u32() >> 1);
        let msg_id = self.take_msg_id();
        protocol::device_message(
            self.s_id,
            msg_id,
            "/ei/initialValues",
            "POST",
            vec![json!({ "edMsgID": ed_msg_id })],
        )
    }

    /// The periodic live update, once the client has read the initial values.
    pub fn tick(&mut self) -> Vec<Value> {
        if !self.ready {
            return Vec::new();
        }
        let change = self.appliance.lock().tick();
        change.map_or_else(Vec::new, |change| vec![self.values_notify(&[change])])
    }

    /// Handles one text frame from the client and returns the frames to send back.
    pub fn handle(&mut self, text: &str) -> Vec<Value> {
        let Some(request) = Request::parse(text) else {
            tracing::warn!("unparseable frame ignored");
            return Vec::new();
        };
        tracing::info!(action = %request.action, resource = %request.resource, "request");
        match (request.action.as_str(), request.resource.as_str()) {
            ("RESPONSE", "/ei/initialValues") => self.on_client_identity(&request),
            ("RESPONSE" | "NOTIFY", _) => Vec::new(),
            ("GET", _) => vec![self.on_get(&request)],
            ("POST", "/ro/values") => self.on_write(&request),
            ("POST", "/ro/selectedProgram" | "/ro/activeProgram") => {
                self.on_program_start(&request)
            }
            ("DELETE", "/ro/activeProgram") => self.on_program_stop(&request),
            _ => {
                tracing::warn!(action = %request.action, resource = %request.resource, "unsupported request");
                vec![protocol::error_response(self.s_id, &request, 404)]
            }
        }
    }

    fn on_client_identity(&mut self, request: &Request) -> Vec<Value> {
        let field = |name: &str| {
            request
                .data
                .first()
                .and_then(|item| item.get(name))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let devices = protocol::registered_devices(&field("deviceName"), &field("deviceID"));
        let msg_id = self.take_msg_id();
        vec![protocol::device_message(
            self.s_id,
            msg_id,
            "/ci/registeredDevices",
            "NOTIFY",
            devices,
        )]
    }

    fn on_get(&mut self, request: &Request) -> Value {
        let data = match request.resource.as_str() {
            "/ci/services" => protocol::services(),
            "/ci/authentication" => {
                let token = URL_SAFE_NO_PAD.encode(self.appliance.lock().random_bytes());
                vec![json!({ "response": token })]
            }
            "/ci/info" => self.info.ci_info(),
            "/iz/info" => self.info.iz_info(),
            "/ni/info" => protocol::ni_info(),
            "/ci/registeredDevices" => protocol::registered_devices("", ""),
            "/ro/allMandatoryValues" | "/ro/values" => {
                self.ready = true;
                self.appliance.lock().mandatory_values()
            }
            "/ro/allDescriptionChanges" => self.appliance.lock().description_changes(),
            _ => {
                tracing::warn!(resource = %request.resource, "GET on an unknown resource");
                return protocol::error_response(self.s_id, request, 404);
            }
        };
        protocol::response(self.s_id, request, Some(data))
    }

    fn on_write(&mut self, request: &Request) -> Vec<Value> {
        let outcome = self.appliance.lock().write(&request.data);
        match outcome {
            Ok(changes) => {
                let items = Value::Array(request.data.clone());
                tracing::info!(%items, "write accepted");
                let mut frames = self.accepted(request, &changes);
                if let Some(&(uid, _)) = changes.first() {
                    frames.extend(self.redescribe(uid));
                }
                frames
            }
            Err(refusal) => self.refused(request, &refusal.0),
        }
    }

    fn on_program_start(&mut self, request: &Request) -> Vec<Value> {
        let outcome = self.appliance.lock().start_program(&request.data);
        match outcome {
            Ok(changes) => {
                let body = Value::Array(request.data.clone());
                tracing::info!(%body, "program start accepted");
                self.accepted(request, &changes)
            }
            Err(refusal) => self.refused(request, &refusal.0),
        }
    }

    fn on_program_stop(&mut self, request: &Request) -> Vec<Value> {
        let outcome = self.appliance.lock().stop_program();
        match outcome {
            Ok(changes) => {
                tracing::info!("program stop accepted");
                self.accepted(request, &changes)
            }
            Err(refusal) => self.refused(request, &refusal.0),
        }
    }

    fn accepted(&mut self, request: &Request, changes: &[Change]) -> Vec<Value> {
        let mut frames = vec![protocol::response(self.s_id, request, None)];
        if !changes.is_empty() {
            frames.push(self.values_notify(changes));
        }
        frames
    }

    fn refused(&self, request: &Request, reason: &str) -> Vec<Value> {
        tracing::warn!(resource = %request.resource, reason, "write refused with code 400");
        vec![protocol::error_response(self.s_id, request, 400)]
    }

    fn redescribe(&mut self, written: u16) -> Option<Value> {
        let item = self.appliance.lock().maybe_redescribe(written)?;
        tracing::info!(item = %item, "description change");
        let msg_id = self.take_msg_id();
        Some(protocol::device_message(
            self.s_id,
            msg_id,
            "/ro/descriptionChange",
            "NOTIFY",
            vec![item],
        ))
    }

    fn values_notify(&mut self, changes: &[Change]) -> Value {
        let items = changes
            .iter()
            .map(|(uid, value)| value_item(*uid, value))
            .collect();
        let msg_id = self.take_msg_id();
        protocol::device_message(self.s_id, msg_id, "/ro/values", "NOTIFY", items)
    }

    const fn take_msg_id(&mut self) -> u64 {
        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);
        msg_id
    }
}
