/* =============================================================================
 * File          : static_routes.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HomeVault-managed static routes: validated CIDR/via/interface/metric,
 *   applied with `ip route replace`, verified against the kernel table,
 *   rolled back on failure, and re-applied at boot from a small JSON store.
 *
 * Dependencies:
 *   - ipnetwork (>= 0.20), serde, crate::utils::process, crate::network
 *
 * Usage:
 *   handlers/network.rs serves `/api/network/routes`; web::run re-applies the
 *   saved routes after startup. See docs/guides/network.md.
 * =============================================================================
 */

//! Static IPv4/IPv6 routes managed by HomeVault.
//!
//! The kernel is the authority: `ip route replace` is atomic, the new route is
//! read back with `ip route show`, and only then is the choice written to
//! `data/static_routes.json`. A save failure restores the previous route. At
//! boot the saved list is re-applied once the network has had time to come up.
//!
//! Route management is deliberately limited to destinations with a non-zero
//! prefix: the default route belongs to the interface configuration screen
//! (`InterfaceConfig::gateway`), and letting a second screen replace it would
//! make the two editors fight.

use std::net::IpAddr;
use std::process::{Command, Output};

use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};

use crate::network::NET_WRITE_TIMEOUT;
use crate::storage::json::JsonStorage;
use crate::utils::process::{run_timed, HOST_PROBE_TIMEOUT};

/// Storage key -> `{DATA_DIRECTORY}/data/static_routes.json`.
const STORE_KEY: &str = "static_routes";

/// How long the boot job waits for the interfaces and their addresses to be
/// up before re-applying the saved routes. A route whose next hop is not
/// reachable yet would be refused by the kernel; the saved list is never
/// deleted because the network was slow.
pub const BOOT_REAPPLY_DELAY: std::time::Duration = std::time::Duration::from_secs(20);

/// One saved route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticRoute {
    /// Destination network in CIDR form, e.g. `192.168.20.0/24`.
    pub destination: String,
    /// Next hop; its address family must match the destination.
    pub via: String,
    /// Linux route metric (lower wins). Omitted when `None`.
    #[serde(default)]
    pub metric: Option<u32>,
    /// Interface the next hop is reachable through.
    pub interface: String,
}

/// A saved route plus whether the kernel currently carries it.
#[derive(Debug, Clone, Serialize)]
pub struct StaticRouteView {
    pub destination: String,
    pub via: String,
    pub metric: Option<u32>,
    pub interface: String,
    pub active: bool,
}

impl StaticRouteView {
    fn from_route(route: &StaticRoute) -> Self {
        Self {
            destination: route.destination.clone(),
            via: route.via.clone(),
            metric: route.metric,
            interface: route.interface.clone(),
            active: is_active(route),
        }
    }
}

fn store() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// Every saved route, in insertion order.
pub fn saved_routes() -> Result<Vec<StaticRoute>, String> {
    match store().load::<Vec<StaticRoute>>(STORE_KEY) {
        Ok(Some(routes)) => Ok(routes),
        Ok(None) => Ok(Vec::new()),
        Err(e) => Err(format!("Could not read the saved static routes: {e}")),
    }
}

fn save_routes(routes: &[StaticRoute]) -> Result<(), String> {
    store()
        .save(STORE_KEY, &routes.to_vec())
        .map_err(|e| format!("Could not save the static routes: {e}"))
}

/// Whether `ip` (iproute2) is installed on this host.
pub fn ip_available() -> bool {
    let mut cmd = Command::new("ip");
    cmd.arg("-V");
    matches!(run_timed(&mut cmd, HOST_PROBE_TIMEOUT), Ok(out) if out.status.success())
}

/// Validate and canonicalise one route. Pure: no host command runs here.
pub fn validate_route(route: &StaticRoute) -> Result<StaticRoute, String> {
    let destination = parse_destination(&route.destination)?;

    let via: IpAddr = route
        .via
        .trim()
        .parse()
        .map_err(|_| "Enter the next-hop address (IPv4 or IPv6).".to_string())?;
    if via.is_unspecified() || via.is_multicast() || via.is_loopback() {
        return Err("The next hop must be a usable unicast address.".to_string());
    }
    if destination.ip().is_ipv4() != via.is_ipv4() {
        return Err(
            "The next hop and the destination must use the same address family.".to_string(),
        );
    }

    let interface = route.interface.trim();
    if !valid_interface_name(interface) {
        return Err(
            "Enter a network interface name (letters, digits, dot, dash, underscore or colon)."
                .to_string(),
        );
    }

    if let Some(metric) = route.metric {
        if metric > u16::MAX as u32 {
            return Err("The metric must be between 0 and 65535.".to_string());
        }
    }

    Ok(StaticRoute {
        destination: destination.to_string(),
        via: via.to_string(),
        metric: route.metric,
        interface: interface.to_string(),
    })
}

fn parse_destination(value: &str) -> Result<IpNetwork, String> {
    let destination: IpNetwork = value.trim().parse().map_err(|_| {
        "Enter a destination network in CIDR form, for example 192.168.20.0/24.".to_string()
    })?;
    if destination.prefix() == 0 {
        return Err(
            "The default route is configured with the interface gateway, not here.".to_string(),
        );
    }
    Ok(destination)
}

fn valid_interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

/// The exact `ip` argv for adding/replacing one route. Pure and tested.
fn route_args(route: &StaticRoute, action: &str) -> Vec<String> {
    let family = if route.destination.contains(':') {
        "-6"
    } else {
        "-4"
    };
    let mut args = vec![
        family.to_string(),
        "route".to_string(),
        action.to_string(),
        route.destination.clone(),
        "via".to_string(),
        route.via.clone(),
        "dev".to_string(),
        route.interface.clone(),
    ];
    if let Some(metric) = route.metric {
        args.push("metric".to_string());
        args.push(metric.to_string());
    }
    args
}

fn show_args(route: &StaticRoute) -> Vec<String> {
    let family = if route.destination.contains(':') {
        "-6"
    } else {
        "-4"
    };
    vec![
        family.to_string(),
        "route".to_string(),
        "show".to_string(),
        route.destination.clone(),
    ]
}

fn run_ip(args: &[String], privileged: bool) -> Result<Output, String> {
    let mut cmd = if privileged {
        let mut cmd = Command::new("sudo");
        cmd.arg("ip");
        cmd
    } else {
        Command::new("ip")
    };
    cmd.args(args);
    run_timed(&mut cmd, NET_WRITE_TIMEOUT)
}

fn require_interface(name: &str) -> Result<(), String> {
    if crate::network::list_interfaces()
        .iter()
        .any(|iface| iface.name == name)
    {
        Ok(())
    } else {
        Err(format!("Unknown network interface: {name}"))
    }
}

fn apply_live(route: &StaticRoute) -> Result<(), String> {
    replace_live(route)?;
    verify_live(route)
}

fn replace_live(route: &StaticRoute) -> Result<(), String> {
    let output = run_ip(&route_args(route, "replace"), true)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "The kernel refused the route: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn verify_live(route: &StaticRoute) -> Result<(), String> {
    let output = run_ip(&show_args(route), false)?;
    if !output.status.success() {
        return Err(format!(
            "Could not read the route back: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let table = String::from_utf8_lossy(&output.stdout);
    let via = format!("via {}", route.via);
    let dev = format!("dev {}", route.interface);
    let present = table
        .lines()
        .any(|line| line.contains(&via) && line.contains(&dev));
    if present {
        Ok(())
    } else {
        Err("The route was not present after it was applied.".to_string())
    }
}

fn remove_live(route: &StaticRoute) -> Result<(), String> {
    let output = run_ip(&route_args(route, "del"), true)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // The route is already gone (reboot before the boot re-apply, interface
    // removed, or an earlier delete): cleanup must still succeed, otherwise a
    // stale saved entry could never be removed from the screen.
    if stderr.contains("No such process") || stderr.contains("Cannot find device") {
        return Ok(());
    }
    Err(format!("The kernel refused to remove the route: {stderr}"))
}

/// Whether the kernel currently carries this saved route.
pub fn is_active(route: &StaticRoute) -> bool {
    verify_live(route).is_ok()
}

/// Add or replace one route, verify it on the wire, then persist it.
pub fn add_route(route: StaticRoute) -> Result<StaticRoute, String> {
    let route = validate_route(&route)?;
    require_interface(&route.interface)?;

    let mut routes = saved_routes()?;
    // One route per destination: a second save replaces the first, which is
    // exactly what `ip route replace` does in the kernel. Keeping two entries
    // would make the list disagree with the wire.
    let previous = routes
        .iter()
        .find(|r| r.destination == route.destination)
        .cloned();

    // A refused `replace` is atomic and changed nothing; a route that was
    // accepted but cannot be read back is rolled back explicitly.
    replace_live(&route)?;
    if let Err(e) = verify_live(&route) {
        match &previous {
            Some(previous) => {
                let _ = apply_live(previous);
            }
            None => {
                let _ = remove_live(&route);
            }
        }
        return Err(e);
    }

    routes.retain(|r| r.destination != route.destination);
    routes.push(route.clone());
    if let Err(e) = save_routes(&routes) {
        // The kernel accepted the route but the disk did not: put the previous
        // state back so "saved" never means less than what is live.
        match &previous {
            Some(previous) => {
                let _ = apply_live(previous);
            }
            None => {
                let _ = remove_live(&route);
            }
        }
        return Err(e);
    }
    Ok(route)
}

/// Remove one saved route (destination + interface identify it).
pub fn delete_route(destination: &str, interface: &str) -> Result<StaticRoute, String> {
    let destination = parse_destination(destination)?.to_string();
    let interface = interface.trim();
    if !valid_interface_name(interface) {
        return Err("Enter a valid network interface name.".to_string());
    }

    let mut routes = saved_routes()?;
    let index = routes
        .iter()
        .position(|r| r.destination == destination && r.interface == interface)
        .ok_or_else(|| format!("No saved route to {destination} on {interface}."))?;
    let removed = routes.remove(index);
    remove_live(&removed)?;
    if let Err(e) = save_routes(&routes) {
        let _ = apply_live(&removed);
        return Err(e);
    }
    Ok(removed)
}

/// The saved routes with an `active` flag, for the UI.
pub fn list_views() -> Result<Vec<StaticRouteView>, String> {
    Ok(saved_routes()?
        .iter()
        .map(StaticRouteView::from_route)
        .collect())
}

/// Re-apply every saved route at boot. Returns one message per failure; never
/// panics, and never removes a route from the store because the network was
/// not ready yet.
pub fn reapply_all() -> Vec<String> {
    let routes = match saved_routes() {
        Ok(routes) => routes,
        Err(e) => return vec![e],
    };
    let mut failures = Vec::new();
    for route in routes {
        let route = match validate_route(&route) {
            Ok(route) => route,
            Err(e) => {
                failures.push(format!("{}: {e}", route.destination));
                continue;
            }
        };
        if let Err(e) = require_interface(&route.interface) {
            failures.push(format!("{}: {e}", route.destination));
            continue;
        }
        if let Err(e) = apply_live(&route) {
            failures.push(format!("{}: {e}", route.destination));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> StaticRoute {
        StaticRoute {
            destination: "192.168.20.0/24".to_string(),
            via: "192.168.1.1".to_string(),
            metric: Some(100),
            interface: "eth0".to_string(),
        }
    }

    #[test]
    fn test_validate_canonicalises_the_destination() {
        let mut r = route();
        r.destination = " 192.168.20.7/24 ".to_string();
        let v = validate_route(&r).unwrap();
        assert_eq!(v.destination, "192.168.20.0/24");
        assert_eq!(v.via, "192.168.1.1");
    }

    #[test]
    fn test_validate_refuses_bad_destinations() {
        for bad in ["", "192.168.1.1", "not-a-network", "0.0.0.0/0", "::/0"] {
            let mut r = route();
            r.destination = bad.to_string();
            assert!(validate_route(&r).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_validate_refuses_a_mismatched_or_unusable_next_hop() {
        let mut r = route();
        r.via = "2001:db8::1".to_string();
        assert!(validate_route(&r).is_err());
        for bad in ["0.0.0.0", "224.0.0.1", "127.0.0.1", "not-an-ip"] {
            let mut r = route();
            r.via = bad.to_string();
            assert!(validate_route(&r).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_validate_refuses_an_interface_name_that_could_be_an_option() {
        for bad in ["", "-eth0", "eth 0", "eth0/1", "a".repeat(16).as_str()] {
            let mut r = route();
            r.interface = bad.to_string();
            assert!(validate_route(&r).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_validate_bounds_the_metric() {
        let mut r = route();
        r.metric = Some(70_000);
        assert!(validate_route(&r).is_err());
        r.metric = Some(0);
        assert!(validate_route(&r).is_ok());
    }

    #[test]
    fn test_route_argv_is_exact_and_never_a_shell_string() {
        let r = validate_route(&route()).unwrap();
        assert_eq!(
            route_args(&r, "replace"),
            vec![
                "-4",
                "route",
                "replace",
                "192.168.20.0/24",
                "via",
                "192.168.1.1",
                "dev",
                "eth0",
                "metric",
                "100",
            ]
        );
        assert_eq!(
            show_args(&r),
            vec!["-4", "route", "show", "192.168.20.0/24"]
        );

        let v6 = validate_route(&StaticRoute {
            destination: "2001:db8:20::/64".to_string(),
            via: "2001:db8::1".to_string(),
            metric: None,
            interface: "eth0".to_string(),
        })
        .unwrap();
        assert_eq!(route_args(&v6, "replace")[0], "-6");
        assert!(!route_args(&v6, "replace").contains(&"metric".to_string()));
    }
}
