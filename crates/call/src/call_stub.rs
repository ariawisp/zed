use std::sync::Arc;

use client::{ChannelId, Client, ParticipantIndex, User, UserStore, proto::{self, PeerId}};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use language::LanguageRegistry;
use fs::Fs;
use project::Project;

pub use crate::call_settings;

// Global handle to a no-op ActiveCall
struct GlobalActiveCall(Entity<ActiveCall>);
impl Global for GlobalActiveCall {}

pub fn init(_client: Arc<Client>, _user_store: Entity<UserStore>, cx: &mut App) {
    let call = cx.new(|_cx| ActiveCall { room: None });
    cx.set_global(GlobalActiveCall(call));
}

#[derive(Clone)]
pub struct ActiveCall {
    room: Option<Entity<Room>>,
}

impl ActiveCall {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalActiveCall>().0.clone()
    }
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalActiveCall>().map(|g| g.0.clone())
    }
    pub fn room(&self) -> Option<&Entity<Room>> { self.room.as_ref() }

    pub fn hang_up(&mut self, _cx: &mut Context<Self>) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    pub fn unshare_project(&mut self, _project: Entity<Project>, _cx: &mut Context<Self>) -> Result<()> {
        Ok(())
    }
    pub fn set_location(&mut self, _project: Option<&Entity<Project>>, _cx: &mut Context<Self>) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
    pub fn share_project(&mut self, _project: Entity<Project>, _cx: &mut Context<Self>) -> Task<Result<u64>> {
        Task::ready(Err(anyhow!("rtc disabled")))
    }
}

pub mod room {
    /// Minimal event set used by workspace/shared_screen.rs and workspace.rs
    pub enum Event {
        ParticipantLocationChanged { participant_id: super::PeerId },
        RemoteVideoTracksChanged { participant_id: super::PeerId },
        RemoteVideoTrackUnsubscribed { sid: String },
    }
}

pub struct Room {
    remote_participants: BTreeMap<u64, RemoteParticipant>,
    empty_followers: Vec<PeerId>,
}
impl EventEmitter<room::Event> for Room {}
impl Room {
    pub fn id(&self) -> u64 { 0 }
    pub fn channel_id(&self) -> Option<ChannelId> { None }
    pub fn is_sharing_project(&self) -> bool { false }
    pub fn remote_participant_for_peer_id(&self, _peer_id: PeerId) -> Option<RemoteParticipant> { None }
    pub fn remote_participants(&self) -> &BTreeMap<u64, RemoteParticipant> { &self.remote_participants }
    pub fn role_for_user(&self, _user_id: u64) -> Option<proto::ChannelRole> { None }
    pub fn followers_for(&self, _leader_id: PeerId, _project_id: u64) -> &[PeerId] { self.empty_followers.as_slice() }
    pub fn most_active_project(&self, _cx: &App) -> Option<(u64, u64)> { None }
    pub fn join_project(
        &mut self,
        _id: u64,
        _language_registry: Arc<LanguageRegistry>,
        _fs: Arc<dyn Fs>,
        _cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Project>>> { Task::ready(Err(anyhow!("rtc disabled"))) }
    pub fn is_sharing_screen(&self) -> bool { false }
    pub fn is_muted(&self) -> bool { false }
    pub fn muted_by_user(&self) -> bool { false }
    pub fn is_deafened(&self) -> Option<bool> { Some(false) }
    pub fn can_use_microphone(&self) -> bool { false }
    pub fn can_share_projects(&self) -> bool { false }
    pub fn toggle_mute(&mut self, _cx: &mut Context<Self>) {}
    pub fn toggle_deafen(&mut self, _cx: &mut Context<Self>) {}
}

#[derive(Clone)]
pub enum ParticipantLocation {
    External,
    UnsharedProject,
    SharedProject { project_id: u64 },
}

#[derive(Clone)]
pub struct RemoteParticipant {
    pub location: ParticipantLocation,
    pub user: Arc<User>,
    pub participant_index: ParticipantIndex,
}

#[derive(Clone)]
pub struct RemoteVideoTrack;
impl RemoteVideoTrack { pub fn sid(&self) -> String { String::new() } }

pub enum RemoteVideoTrackViewEvent { Close }

pub struct RemoteVideoTrackView;
impl RemoteVideoTrackView {
    pub fn new(_track: RemoteVideoTrack, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
    pub fn clone(&self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> Entity<Self> {
        cx.new(|_cx| Self)
    }
}
impl EventEmitter<RemoteVideoTrackViewEvent> for RemoteVideoTrackView {}
