//! A blocking client for GlazeWM's IPC server, with the few container types rallypoint reads. GlazeWM's own
//! `wm-ipc-client` is an unpublished crate that pulls in tokio and an older `windows`, so the types are copied here.

use std::net::TcpStream;
use std::sync::mpsc::Sender;
use std::thread;

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::error::Error;

const URL: &str = "ws://127.0.0.1:6123";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub name: String,
    pub has_focus: bool,
    /// Shown on its monitor, which holds one displayed workspace at a time.
    pub is_displayed: bool,
    pub children: Vec<Container>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Container {
    Window(Window),
    Split { children: Vec<Container> },
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// The container ID that scopes a command to this window. GlazeWM gives a new one each time it manages a window.
    pub id: String,
    pub handle: isize,
    pub title: String,
    pub class_name: String,
    pub process_name: String,
    pub state: WindowState,
}

#[derive(Deserialize, Clone)]
pub struct WindowState {
    #[serde(rename = "type")]
    pub kind: State,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Tiling,
    Floating,
    Minimized,
    Fullscreen,
}

impl State {
    /// The GlazeWM command that puts a window in this state.
    fn command(self) -> &'static str {
        match self {
            State::Tiling => "set-tiling",
            State::Floating => "set-floating",
            State::Minimized => "set-minimized",
            State::Fullscreen => "set-fullscreen",
        }
    }
}

impl Workspace {
    /// Every window on the workspace, in tree order, however deep its split containers nest.
    pub fn windows(&self) -> Vec<Window> {
        flatten(&self.children)
    }
}

fn flatten(children: &[Container]) -> Vec<Window> {
    children
        .iter()
        .flat_map(|child| match child {
            Container::Window(window) => vec![window.clone()],
            Container::Split { children } => flatten(children),
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply<'a> {
    client_message: String,
    #[serde(borrow)]
    data: Option<&'a RawValue>,
    error: Option<String>,
    success: bool,
}

#[derive(Deserialize)]
struct Workspaces {
    workspaces: Vec<Workspace>,
}

pub struct Client {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl Client {
    pub fn connect() -> Result<Client, Error> {
        let (socket, _) = tungstenite::connect(URL).map_err(|source| Error::GlazeConnect {
            url: URL,
            source: Box::new(source),
        })?;
        Ok(Client { socket })
    }

    pub fn workspaces(&mut self) -> Result<Vec<Workspace>, Error> {
        Ok(self.query::<Workspaces>("query workspaces")?.workspaces)
    }

    /// Moves window `id` to `workspace`, which GlazeWM creates when it is not open.
    pub fn move_to_workspace(&mut self, id: &str, workspace: &str) -> Result<(), Error> {
        self.query::<IgnoredAny>(&format!("command --id {id} move --workspace {workspace}"))?;
        Ok(())
    }

    pub fn set_state(&mut self, id: &str, state: State) -> Result<(), Error> {
        self.query::<IgnoredAny>(&format!("command --id {id} {}", state.command()))?;
        Ok(())
    }

    /// Focuses `workspace` and shows it on its monitor. With `toggle_workspace_on_refocus` set, focusing the focused
    /// workspace jumps to the previous one instead, so callers focus only a workspace that is not focused.
    pub fn focus_workspace(&mut self, workspace: &str) -> Result<(), Error> {
        self.query::<IgnoredAny>(&format!("command focus --workspace {workspace}"))?;
        Ok(())
    }

    /// Subscribes to `events` (GlazeWM event names such as `window_managed`) and hands the connection over to them.
    pub fn subscribe(mut self, events: &[&str]) -> Result<Subscription, Error> {
        let message = format!("sub -e {}", events.join(" "));
        self.query::<IgnoredAny>(&message)?;
        Ok(Subscription {
            socket: self.socket,
            message,
        })
    }

    /// Sends `message` and decodes the `data` of the reply to it.
    fn query<T: DeserializeOwned>(&mut self, message: &str) -> Result<T, Error> {
        let socket_error = |source| Error::GlazeSocket {
            message: message.to_string(),
            source: Box::new(source),
        };
        self.socket
            .send(Message::Text(message.into()))
            .map_err(socket_error)?;
        loop {
            let text = match self.socket.read().map_err(socket_error)? {
                Message::Text(text) => text,
                _ => continue,
            };
            let reply_error = |source| Error::GlazeReply {
                message: message.to_string(),
                reply: text.to_string(),
                source,
            };
            let reply: Reply = serde_json::from_str(&text).map_err(reply_error)?;
            if reply.client_message != message {
                continue;
            }
            if !reply.success {
                return Err(Error::GlazeRefused {
                    message: message.to_string(),
                    reason: reply.error,
                });
            }
            let data = reply.data.map_or("null", RawValue::get);
            return serde_json::from_str(data).map_err(reply_error);
        }
    }
}

#[derive(Deserialize, PartialEq, Debug)]
#[serde(tag = "eventType", rename_all = "snake_case")]
pub enum Event {
    ApplicationExiting,
    /// Any subscribed event other than GlazeWM exiting.
    #[serde(other)]
    Changed,
}

#[derive(Deserialize)]
struct EventMessage {
    data: Option<Event>,
    error: Option<String>,
    success: bool,
}

/// A connection that only receives the events it subscribed to.
pub struct Subscription {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    message: String,
}

impl Subscription {
    /// Sends each event, wrapped by `wrap`, to `signals` from a new thread. Stops after an error, after GlazeWM
    /// exiting, or once `signals` has no receiver.
    pub fn forward<T: Send + 'static>(
        mut self,
        signals: Sender<T>,
        wrap: fn(Result<Event, Error>) -> T,
    ) {
        thread::spawn(move || {
            loop {
                let event = self.next_event();
                let last = !matches!(event, Ok(Event::Changed));
                if signals.send(wrap(event)).is_err() || last {
                    return;
                }
            }
        });
    }

    /// Blocks until the next event arrives.
    fn next_event(&mut self) -> Result<Event, Error> {
        loop {
            let text = match self.socket.read() {
                Ok(Message::Text(text)) => text,
                Ok(_) => continue,
                Err(source) => {
                    return Err(Error::GlazeSocket {
                        message: self.message.clone(),
                        source: Box::new(source),
                    });
                }
            };
            let event: EventMessage =
                serde_json::from_str(&text).map_err(|source| Error::GlazeReply {
                    message: self.message.clone(),
                    reply: text.to_string(),
                    source,
                })?;
            return match (event.success, event.data) {
                (true, Some(data)) => Ok(data),
                _ => Err(Error::GlazeRefused {
                    message: self.message.clone(),
                    reason: event.error,
                }),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_tells_exiting_from_any_other_event() -> Result<(), serde_json::Error> {
        let exiting: EventMessage = serde_json::from_str(
            r#"{"messageType":"event_subscription","subscriptionId":"s","success":true,"error":null,
                "data":{"eventType":"application_exiting"}}"#,
        )?;
        let managed: EventMessage = serde_json::from_str(
            r#"{"messageType":"event_subscription","subscriptionId":"s","success":true,"error":null,
                "data":{"eventType":"window_managed","managedWindow":{"id":"x"}}}"#,
        )?;
        assert_eq!(exiting.data, Some(Event::ApplicationExiting));
        assert_eq!(managed.data, Some(Event::Changed));
        Ok(())
    }

    #[test]
    fn windows_walks_nested_splits_in_order() -> Result<(), serde_json::Error> {
        let workspace: Workspace = serde_json::from_str(
            r#"{"type":"workspace","name":"2","hasFocus":false,"isDisplayed":true,"children":[
                {"type":"window","id":"x","handle":1,"title":"a","className":"c","processName":"p",
                 "state":{"type":"tiling"}},
                {"type":"split","children":[
                    {"type":"window","id":"y","handle":2,"title":"b","className":"c","processName":"p",
                     "state":{"type":"floating","centered":true}}
                ]}
            ]}"#,
        )?;
        let handles: Vec<isize> = workspace.windows().iter().map(|w| w.handle).collect();
        assert_eq!(handles, vec![1, 2]);
        Ok(())
    }
}
