use crate::terminal::enrich_terminal_windows;
use crate::windowing::registry::BackendProbe;
use crate::windowing::types::{WindowBounds, WindowInfo};
use anyhow::{bail, Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

pub const X11_EWMH_BACKEND: &str = "x11-ewmh";

pub fn probe() -> BackendProbe {
    match connect() {
        Ok((connection, screen)) => {
            let root = connection.setup().roots[screen].root;
            let result = atom(&connection, "_NET_CLIENT_LIST_STACKING")
                .and_then(|property| window_property(&connection, root, property));
            let ok = result.is_ok();
            BackendProbe {
                id: X11_EWMH_BACKEND,
                ok,
                can_list_windows: ok,
                can_focus_apps: ok,
                can_focus_windows: ok,
                detail: match result {
                    Ok(windows) => format!(
                        "EWMH window manager is available on DISPLAY ({} windows)",
                        windows.len()
                    ),
                    Err(error) => format!("EWMH window discovery unavailable: {error:#}"),
                },
            }
        }
        Err(error) => BackendProbe {
            id: X11_EWMH_BACKEND,
            ok: false,
            can_list_windows: false,
            can_focus_apps: false,
            can_focus_windows: false,
            detail: error.to_string(),
        },
    }
}

pub fn list_windows() -> Result<Vec<WindowInfo>> {
    let (connection, screen) = connect()?;
    let root = connection.setup().roots[screen].root;
    let clients = atom(&connection, "_NET_CLIENT_LIST_STACKING")
        .and_then(|property| window_property(&connection, root, property))?;
    let active = atom(&connection, "_NET_ACTIVE_WINDOW")
        .ok()
        .and_then(|property| window_property(&connection, root, property).ok())
        .and_then(|windows| windows.first().copied());
    let current_desktop = atom(&connection, "_NET_CURRENT_DESKTOP")
        .ok()
        .and_then(|property| cardinal_property(&connection, root, property).ok());
    let hidden_atom = atom(&connection, "_NET_WM_STATE_HIDDEN").ok();
    let state_atom = atom(&connection, "_NET_WM_STATE").ok();
    let pid_atom = atom(&connection, "_NET_WM_PID").ok();
    let desktop_atom = atom(&connection, "_NET_WM_DESKTOP").ok();
    let name_atom = atom(&connection, "_NET_WM_NAME").ok();
    let utf8_atom = atom(&connection, "UTF8_STRING").ok();

    let mut windows = Vec::new();
    for window in clients {
        let geometry = match connection.get_geometry(window)?.reply() {
            Ok(value) if value.width > 0 && value.height > 0 => value,
            _ => continue,
        };
        let translated = connection
            .translate_coordinates(window, root, 0, 0)?
            .reply()
            .ok();
        let (instance, class) = wm_class(&connection, window).unwrap_or_default();
        let title = match (name_atom, utf8_atom) {
            (Some(property), Some(property_type)) => {
                string_property(&connection, window, property, property_type).ok()
            }
            _ => None,
        }
        .or_else(|| {
            string_property(
                &connection,
                window,
                AtomEnum::WM_NAME.into(),
                AtomEnum::STRING.into(),
            )
            .ok()
        });
        let workspace = desktop_atom
            .and_then(|property| cardinal_property(&connection, window, property).ok())
            .and_then(|value| (value != u32::MAX).then_some(value as i32));
        let states = state_atom
            .and_then(|property| atom_property(&connection, window, property).ok())
            .unwrap_or_default();
        let hidden = hidden_atom.is_some_and(|hidden| states.contains(&hidden));
        let focused = active == Some(window);
        let visible_workspace = workspace
            .zip(current_desktop)
            .is_none_or(|(window_desktop, desktop)| window_desktop == desktop as i32);

        windows.push(WindowInfo {
            window_id: u64::from(window),
            title,
            app_id: clean(instance),
            wm_class: clean(class),
            pid: pid_atom
                .and_then(|property| cardinal_property(&connection, window, property).ok()),
            bounds: Some(WindowBounds {
                x: translated.as_ref().map(|value| i32::from(value.dst_x)),
                y: translated.as_ref().map(|value| i32::from(value.dst_y)),
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
            }),
            workspace,
            focused,
            hidden: hidden || !visible_workspace,
            client_type: Some("x11".to_string()),
            backend: X11_EWMH_BACKEND.to_string(),
            terminal: None,
        });
    }
    enrich_terminal_windows(&mut windows);
    Ok(windows)
}

pub fn activate_window(window_id: u64) -> Result<()> {
    let window = u32::try_from(window_id).context("X11 window id exceeds 32 bits")?;
    let (connection, screen) = connect()?;
    let root = connection.setup().roots[screen].root;
    let active_atom = atom(&connection, "_NET_ACTIVE_WINDOW")?;
    let event = ClientMessageEvent::new(
        32,
        window,
        active_atom,
        ClientMessageData::from([1, 0, 0, 0, 0]),
    );
    connection
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?
        .check()
        .context("window manager rejected _NET_ACTIVE_WINDOW")?;
    connection.flush()?;
    Ok(())
}

fn connect() -> Result<(RustConnection, usize)> {
    x11rb::connect(None).context("failed to connect to X11 DISPLAY")
}

fn atom(connection: &RustConnection, name: &str) -> Result<Atom> {
    Ok(connection
        .intern_atom(false, name.as_bytes())?
        .reply()?
        .atom)
}

fn window_property(
    connection: &RustConnection,
    window: Window,
    property: Atom,
) -> Result<Vec<Window>> {
    let reply = connection
        .get_property(false, window, property, AtomEnum::WINDOW, 0, u32::MAX)?
        .reply()?;
    reply
        .value32()
        .map(|values| values.collect())
        .context("EWMH property was not a WINDOW list")
}

fn atom_property(connection: &RustConnection, window: Window, property: Atom) -> Result<Vec<Atom>> {
    let reply = connection
        .get_property(false, window, property, AtomEnum::ATOM, 0, u32::MAX)?
        .reply()?;
    reply
        .value32()
        .map(|values| values.collect())
        .context("EWMH property was not an ATOM list")
}

fn cardinal_property(connection: &RustConnection, window: Window, property: Atom) -> Result<u32> {
    let reply = connection
        .get_property(false, window, property, AtomEnum::CARDINAL, 0, 1)?
        .reply()?;
    reply
        .value32()
        .and_then(|mut values| values.next())
        .context("EWMH property had no CARDINAL value")
}

fn string_property(
    connection: &RustConnection,
    window: Window,
    property: Atom,
    property_type: Atom,
) -> Result<String> {
    let reply = connection
        .get_property(false, window, property, property_type, 0, u32::MAX)?
        .reply()?;
    if reply.value.is_empty() {
        bail!("string property was empty");
    }
    Ok(String::from_utf8_lossy(&reply.value)
        .trim_matches('\0')
        .trim()
        .to_string())
}

fn wm_class(connection: &RustConnection, window: Window) -> Result<(String, String)> {
    let reply = connection
        .get_property(
            false,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            u32::MAX,
        )?
        .reply()?;
    parse_wm_class(&reply.value).context("WM_CLASS was empty")
}

pub(crate) fn parse_wm_class(value: &[u8]) -> Option<(String, String)> {
    let mut fields = value
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let instance = String::from_utf8_lossy(fields.next()?).trim().to_string();
    let class = String::from_utf8_lossy(fields.next().unwrap_or_default())
        .trim()
        .to_string();
    Some((instance, class))
}

fn clean(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_wm_class_pair() {
        assert_eq!(
            parse_wm_class(b"libreoffice\0LibreOffice\0"),
            Some(("libreoffice".to_string(), "LibreOffice".to_string()))
        );
    }
}
