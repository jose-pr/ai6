//! Cables to a Liaison peer (`a2a:<peer>`, `a2a:<peer>/<addressee>`): Liaison reads them from the
//! Registry as the peer's principal, sends each as an A2A message, and answers its sender with the
//! peer's reply as a Cable. A peer is reached on the link it holds open to the gateway, else by
//! dialing its `url`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ai6::directorates::registry::cables::{CableDelivery, CableEndpoint, liaison_peer};
use coqui_a2a::model::{GetTaskRequest, Message, Role, SendMessageRequest, SendMessageResponse};
use coqui_a2a::{Client, ClientError, ClientTimeouts};
use coqui_agentic_peer::auth::{Identity, Proof, Sealed, Trust, prove_out};
use coqui_agentic_peer::mux::{Limits, LinkState, Mux, Side};
use coqui_web_channel::websocket::{Dialer, Frames};
use coqui_web_channel::{Channel, Dialer as _, Kind};
use serde_json::Value;

use super::{ADDRESSEE_MEMBER, Liaison, LiaisonPeer, now};

/// The time a dial and its proof have.
const DIAL_WAIT: Duration = Duration::from_secs(10);

/// What the outbound side shares between the links peers hold and the dialer.
pub(super) struct Outbound {
    liaison: Liaison,
    identity: Identity,
    /// Cables being carried now, so a link and the dialer never carry one twice.
    carrying: Mutex<BTreeSet<String>>,
    /// Peers holding a link open to the gateway, by name: the dialer leaves them to it.
    linked: Mutex<BTreeMap<String, usize>>,
}

impl Outbound {
    pub(super) fn new(liaison: Liaison, identity: Identity) -> Arc<Self> {
        Arc::new(Self {
            liaison,
            identity,
            carrying: Mutex::new(BTreeSet::new()),
            linked: Mutex::new(BTreeMap::new()),
        })
    }

    /// Carries what waits for the peer the link proved, over that link, for as long as it is up.
    pub(super) fn on_link<C: Channel>(&self, peer_id: &str, mux: &Mux<'_, C>, stop: &AtomicBool) {
        let Some(peer) = self.liaison.peer_by_id(peer_id).cloned() else {
            return;
        };
        *self.linked().entry(peer.name.clone()).or_default() += 1;
        while matches!(mux.link_state(), LinkState::Up) && !stop.load(Ordering::SeqCst) {
            for delivery in self.waiting(&peer) {
                self.carry(&peer, mux, delivery);
            }
            std::thread::sleep(self.liaison.inner.config.poll);
        }
        let mut linked = self.linked();
        if let Some(count) = linked.get_mut(&peer.name) {
            *count -= 1;
            if *count == 0 {
                linked.remove(&peer.name);
            }
        }
    }

    /// Dials each peer with a `url` that has something waiting and no link of its own up, until
    /// `stop`.
    pub(super) fn dial_loop(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            for peer in self.liaison.inner.config.peers.values() {
                if peer.url.is_none() || self.linked().contains_key(&peer.name) {
                    continue;
                }
                let waiting = self.waiting(peer);
                if waiting.is_empty() {
                    continue;
                }
                if let Err(error) = self.dial(peer, waiting) {
                    eprintln!(
                        "universal-exports: Liaison could not reach peer {}: {error}",
                        peer.name
                    );
                }
            }
            std::thread::sleep(self.liaison.inner.config.poll);
        }
    }

    fn linked(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, usize>> {
        self.linked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn dial(&self, peer: &LiaisonPeer, waiting: Vec<CableDelivery>) -> Result<(), String> {
        let url = peer.url.as_ref().ok_or("no url")?;
        let server = peer
            .id
            .as_deref()
            .ok_or("no peer id")?
            .parse()
            .map_err(|_| "not a peer id")?;
        let proof = Proof::new(self.identity.clone()).with_responders(Trust::pinned([server]));
        let mut dialer = Dialer::new().with_frames(Frames::Binary);
        if url.scheme() == "wss" {
            dialer = dialer.with_tls(public_roots()?);
        } else if self.liaison.inner.config.plain_beyond_loopback {
            dialer = dialer.with_plain_beyond_loopback();
        }
        let channel = dialer
            .dial(Kind::WebSocket, url, DIAL_WAIT)
            .map_err(|error| error.to_string())?;
        let limits = Limits::default();
        let established = prove_out(
            &proof,
            &channel,
            DIAL_WAIT,
            proof.method().record_length(limits.max_message),
        )
        .map_err(|error| error.to_string())?;
        let sealed = Sealed::new(&channel, established.session);
        Mux::run(&sealed, Side::Dialed, limits, None, |mux| {
            for delivery in waiting {
                self.carry(peer, mux, delivery);
            }
        });
        sealed.close();
        channel.close();
        Ok(())
    }

    /// The Cables waiting for `peer`, from each Operation where its principal may read them.
    fn waiting(&self, peer: &LiaisonPeer) -> Vec<CableDelivery> {
        let liaison = &self.liaison;
        let (Ok(registry), Ok(snapshot)) = (liaison.registry(), liaison.snapshot()) else {
            return Vec::new();
        };
        let Ok(relay) = liaison.relay(&snapshot, Vec::new()) else {
            return Vec::new();
        };
        let page = registry.limits().cables.page.or_max();
        let carrying = self
            .carrying
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut waiting = Vec::new();
        for operation in liaison.operations() {
            if !liaison.allowed(peer, "cable.read", &operation) {
                continue;
            }
            let Ok(principal) = Liaison::principal_for(peer, &proof_of(peer), &operation, None)
            else {
                continue;
            };
            let Ok(deliveries) = relay.inbox_from(&principal, &operation, page) else {
                continue;
            };
            waiting.extend(deliveries.into_iter().filter(|delivery| {
                !carrying.contains(&delivery.cable_id)
                    && delivery.peer.is_none()
                    && delivery
                        .target
                        .name()
                        .and_then(liaison_peer)
                        .is_some_and(|(name, _)| name == peer.name)
            }));
        }
        waiting
    }

    /// Sends one Cable to the peer as an A2A message and answers its sender with what came back.
    fn carry<C: Channel>(&self, peer: &LiaisonPeer, mux: &Mux<'_, C>, delivery: CableDelivery) {
        if !self
            .carrying
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(delivery.cable_id.clone())
        {
            return;
        }
        let outcome = self.exchange(peer, mux, &delivery);
        let (text, outcome) = match outcome {
            Ok(text) => (text, "answered"),
            Err(text) => (text, "failed"),
        };
        let answered = self.answer(peer, &delivery, text);
        self.liaison.audit(
            peer,
            "outbound",
            &delivery.from_operation,
            &format!(
                "cable {} from agent:{} {outcome}{}",
                delivery.cable_id,
                delivery.from_agent,
                if answered { "" } else { ", no reply Cable" }
            ),
        );
        self.carrying
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&delivery.cable_id);
    }

    /// The peer's answer to one Cable as text, or why there is none.
    fn exchange<C: Channel>(
        &self,
        peer: &LiaisonPeer,
        mux: &Mux<'_, C>,
        delivery: &CableDelivery,
    ) -> Result<String, String> {
        let left = Duration::try_from_secs_f64(delivery.expires_at - now()).unwrap_or_default();
        if left.is_zero() {
            return Err("[Liaison] the Cable expired before the peer was reached".to_owned());
        }
        let mut timeouts = ClientTimeouts::default();
        timeouts.answer = left;
        let client = Client::new(mux).with_timeouts(timeouts);
        let mut message =
            Message::text(Role::User, delivery.cable_id.clone(), delivery.body.clone());
        let mut metadata = serde_json::Map::new();
        if let Some((_, Some(addressee))) = delivery.target.name().and_then(liaison_peer) {
            metadata.insert(
                ADDRESSEE_MEMBER.to_owned(),
                Value::String(addressee.to_owned()),
            );
        }
        metadata.insert(
            "from".to_owned(),
            Value::String(format!(
                "agent:{}/{}",
                delivery.from_operation, delivery.from_agent
            )),
        );
        message.metadata = Some(metadata);
        let failed = |error: ClientError| match error {
            ClientError::NotGranted(_) | ClientError::Revoked(_) => {
                format!(
                    "[Liaison] peer {} does not take A2A from this gateway",
                    peer.name
                )
            }
            ClientError::Remote(error) => format!("[A2A error {}] {}", error.code, error.message),
            other => format!(
                "[Liaison] the exchange with peer {} failed: {other}",
                peer.name
            ),
        };
        let mut task = match client.send_message(&SendMessageRequest::new(message)) {
            Ok(SendMessageResponse::Message(reply)) => return Ok(reply.joined_text()),
            Ok(SendMessageResponse::Task(task)) => task,
            Ok(_) => return Err("[Liaison] the peer answered with nothing".to_owned()),
            Err(error) => return Err(failed(error)),
        };
        // A task still running is followed until it ends, or the Cable does.
        while !task.status.state.is_terminal() && delivery.expires_at > now() {
            std::thread::sleep(self.liaison.inner.config.poll);
            task = client
                .get_task(&GetTaskRequest {
                    id: task.id.clone(),
                    ..GetTaskRequest::default()
                })
                .map_err(failed)?;
        }
        let text = task
            .status
            .message
            .as_ref()
            .map(Message::joined_text)
            .filter(|text| !text.is_empty())
            .or_else(|| {
                task.artifacts.as_ref().map(|artifacts| {
                    artifacts
                        .iter()
                        .flat_map(|artifact| artifact.parts.iter())
                        .filter_map(|part| part.text.as_deref())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            })
            .unwrap_or_default();
        if task.status.state == coqui_a2a::model::TaskState::Completed {
            Ok(text)
        } else {
            Err(format!("[A2A task {}] {text}", task.status.state))
        }
    }

    /// Replies to the Cable's sender as the peer, and takes the Cable off the peer's inbox.
    fn answer(&self, peer: &LiaisonPeer, delivery: &CableDelivery, text: String) -> bool {
        let liaison = &self.liaison;
        let Ok(snapshot) = liaison.snapshot() else {
            return false;
        };
        let Ok(sender) = CableEndpoint::try_new(
            delivery.from_agent.clone(),
            delivery.from_operation.clone(),
            delivery.from_theater.clone(),
            delivery.from_station.clone(),
            delivery.session_id.clone(),
            delivery.mission_id.clone(),
        ) else {
            return false;
        };
        let (Ok(relay), Ok(principal)) = (
            liaison.relay(&snapshot, vec![sender]),
            Liaison::principal_for(peer, &proof_of(peer), &delivery.from_operation, None),
        ) else {
            return false;
        };
        let text = if text.trim().is_empty() {
            "[Liaison] the peer's answer held no text".to_owned()
        } else {
            text
        };
        let body_bytes = relay.body_bytes();
        let text = if body_bytes.allows(text.len()) {
            text
        } else {
            let mut cut = text;
            super::cut(&mut cut, body_bytes.or_max());
            cut
        };
        let replied = relay
            .reply(
                &principal,
                &delivery.cable_id,
                text,
                format!("a2a-reply-{}", delivery.cable_id),
            )
            .is_ok();
        let _ = relay.acknowledge(&principal, &delivery.cable_id);
        replied
    }
}

/// The public roots, for a `wss://` peer: its proof names it, and TLS hides the link from the
/// network.
fn public_roots() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        roots
            .add(certificate.clone())
            .map_err(|error| error.to_string())?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| error.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// What a peer's Cables are stamped with when Liaison carries them: its key, else its principal.
fn proof_of(peer: &LiaisonPeer) -> String {
    peer.id.clone().unwrap_or_else(|| peer.principal.clone())
}
