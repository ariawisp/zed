#![allow(dead_code)]

#[cfg(feature = "rtc")]
use crate::{
    ItemNavHistory, WorkspaceId,
    item::{Item, ItemEvent},
};
#[cfg(feature = "rtc")]
use call::{RemoteVideoTrack, RemoteVideoTrackView, Room, RemoteVideoTrackViewEvent, room};
#[cfg(feature = "rtc")]
use client::{User, proto::PeerId};
use gpui::{
    AppContext as _, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    ParentElement, Render, SharedString, Styled, div,
};
#[cfg(feature = "rtc")]
use std::sync::Arc;
use ui::{Icon, IconName, prelude::*};
#[cfg(not(feature = "rtc"))]
use crate::Item;

pub enum Event { Close }

#[cfg(feature = "rtc")]
pub struct SharedScreen {
    pub peer_id: PeerId,
    user: Arc<User>,
    nav_history: Option<ItemNavHistory>,
    view: Entity<RemoteVideoTrackView>,
    focus: FocusHandle,
}

#[cfg(not(feature = "rtc"))]
pub struct SharedScreen { pub peer_id: u64, focus: FocusHandle }

#[cfg(feature = "rtc")]
impl SharedScreen {
    pub fn new(
        track: RemoteVideoTrack,
        peer_id: PeerId,
        user: Arc<User>,
        room: Entity<Room>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let my_sid = track.sid();
        cx.subscribe(&room, move |_, _, ev, cx| {
            if let call::room::Event::RemoteVideoTrackUnsubscribed { sid } = ev
                && sid == &my_sid
            {
                cx.emit(Event::Close)
            }
        })
        .detach();

        let view = cx.new(|cx| RemoteVideoTrackView::new(track.clone(), window, cx));
        cx.subscribe(&view, |_, _, ev, cx| match ev {
            call::RemoteVideoTrackViewEvent::Close => cx.emit(Event::Close),
        })
        .detach();
        Self {
            view,
            peer_id,
            user,
            nav_history: Default::default(),
            focus: cx.focus_handle(),
        }
    }
}

impl EventEmitter<Event> for SharedScreen {}

impl Focusable for SharedScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
#[cfg(feature = "rtc")]
impl Render for SharedScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(cx.theme().colors().editor_background)
            .track_focus(&self.focus)
            .key_context("SharedScreen")
            .size_full()
            .child(self.view.clone())
    }
}

#[cfg(feature = "rtc")]
impl Item for SharedScreen {
    type Event = Event;

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some(format!("{}'s screen", self.user.github_login).into())
    }

    fn deactivated(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(nav_history) = self.nav_history.as_mut() {
            nav_history.push::<()>(None, cx);
        }
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Screen))
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("{}'s screen", self.user.github_login).into()
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }

    fn set_nav_history(
        &mut self,
        history: ItemNavHistory,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.nav_history = Some(history);
    }

    fn clone_on_split(
        &self,
        _workspace_id: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Self>> {
        #[cfg(feature = "rtc")]
        {
            return Some(cx.new(|cx| Self {
                view: self.view.update(cx, |view, cx| view.clone(window, cx)),
                peer_id: self.peer_id,
                user: self.user.clone(),
                nav_history: Default::default(),
                focus: cx.focus_handle(),
            }));
        }
        #[cfg(not(feature = "rtc"))]
        {
            return Some(cx.new(|cx| Self { peer_id: self.peer_id, focus: cx.focus_handle() }));
        }
    }

    fn to_item_events(event: &Self::Event, mut f: impl FnMut(ItemEvent)) {
        match event {
            Event::Close => f(ItemEvent::CloseItem),
        }
    }
}

#[cfg(not(feature = "rtc"))]
impl Render for SharedScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(cx.theme().colors().editor_background)
            .track_focus(&self.focus)
            .key_context("SharedScreen")
            .size_full()
    }
}

#[cfg(not(feature = "rtc"))]
impl Item for SharedScreen {
    type Event = Event;
    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Shared Screen".into()
    }
}
