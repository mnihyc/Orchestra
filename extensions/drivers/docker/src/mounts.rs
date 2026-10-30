//! Path locations as bind mounts and volume locations as named volumes, at their targets, for an environment.
//!
//! The mounts of an environment arrive in the runtime request, each with the location this host has for it. A
//! `path` becomes a bind mount of that directory and a `volume` a named volume, both at the mount's target;
//! read-only is enforced by the mount itself. A mount that was asked to be a private view becomes one when the
//! host can make one ([`crate::overlay`]), and is attached read-only otherwise: it is never shared instead. A
//! mount that cannot be attached (its target collides with a place of the environment's own or with a nearer
//! mount, its directory is not there) is left out and said so, and the connector reports it unavailable.
//!
//! [`plan`] decides all this without touching the daemon, so that every rule has a test that needs none.

use bollard::models::{Mount, MountType, MountVolumeOptions};
use orch_driver_contract::mounts::{
    AttachedMount, MountKind, MountLayer, MountMode, MountSpec, TargetFault, attach_mode,
    check_targets,
};

use crate::volumes::is_volume_name;

/// A private view that is still to be made: the mount, and where its changes are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The mount.
    pub mount: MountSpec,
    /// The volume that keeps the environment's own changes.
    pub layer: MountLayer,
}

/// What became of the mounts a request asked for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Planned {
    /// The mounts that can be attached as they are.
    pub direct: Vec<Mount>,
    /// The private views to be made, which become mounts when they are.
    pub views: Vec<View>,
    /// The mounts as they are attached, with the mode each got: what the connector is told.
    pub attached: Vec<AttachedMount>,
    /// The mounts that are not attached, with the reason in words.
    pub left_out: Vec<(String, String)>,
}

/// Whether a path can be written into the options of an overlay mount, where `,` ends an option and `:` ends a
/// directory and nothing escapes either.
#[must_use]
pub fn is_overlay_safe(path: &str) -> bool {
    path.starts_with('/')
        && !path
            .chars()
            .any(|c| c.is_control() || matches!(c, ',' | ':' | '\\'))
}

/// A bind mount of a directory of the host.
#[must_use]
pub fn bind(source: &str, target: &str, read_only: bool) -> Mount {
    Mount {
        typ: Some(MountType::BIND),
        source: Some(source.to_owned()),
        target: Some(target.to_owned()),
        read_only: Some(read_only),
        ..Mount::default()
    }
}

/// A named volume.
#[must_use]
pub fn volume(name: &str, target: &str, read_only: bool) -> Mount {
    Mount {
        typ: Some(MountType::VOLUME),
        source: Some(name.to_owned()),
        target: Some(target.to_owned()),
        read_only: Some(read_only),
        ..Mount::default()
    }
}

/// A named volume that the image's own files are never copied into. A volume of Orch's own (the workspace, the
/// state, the files of a run) must be what the driver made it, belonging to the unit's user: the daemon copies what
/// an image has at the mount's place into a volume that is empty, and the user and group of that place with it.
#[must_use]
pub fn volume_without_image(name: &str, target: &str) -> Mount {
    Mount {
        volume_options: Some(MountVolumeOptions {
            no_copy: Some(true),
            ..MountVolumeOptions::default()
        }),
        ..volume(name, target, false)
    }
}

/// One file of a volume, mounted over a file of the unit (`subpath` is the file's place in the volume): how a unit
/// whose runtime cannot reach the daemon's own resolver is given a resolver file of its own.
#[must_use]
pub fn volume_file(name: &str, subpath: &str, target: &str) -> Mount {
    Mount {
        volume_options: Some(MountVolumeOptions {
            subpath: Some(subpath.to_owned()),
            no_copy: Some(true),
            ..MountVolumeOptions::default()
        }),
        ..volume(name, target, true)
    }
}

fn fault_words(fault: &TargetFault) -> String {
    match fault {
        TargetFault::Invalid(error) => error.to_string(),
        TargetFault::Reserved => {
            "its target is, contains or lies inside a place of the environment's own".to_owned()
        }
        TargetFault::Taken => "its target is the target of a nearer mount".to_owned(),
    }
}

/// Decides what is attached. `reserved` are the places no mount may be on; `private_views` is whether this host
/// can make a view; `directory_exists` says whether a directory of the host is there (it is not asked for a
/// volume).
#[must_use]
pub fn plan(
    mounts: &[MountSpec],
    reserved: &[&str],
    private_views: bool,
    directory_exists: &dyn Fn(&str) -> bool,
) -> Planned {
    let pairs: Vec<(&str, &str)> = mounts
        .iter()
        .map(|mount| (mount.name.as_str(), mount.target.as_str()))
        .collect();
    let refused = check_targets(&pairs, reserved);
    let mut planned = Planned::default();
    for mount in mounts {
        if let Some((_, fault)) = refused
            .iter()
            .find(|(name, _)| *name == mount.name.as_str())
        {
            planned
                .left_out
                .push((mount.name.clone(), fault_words(fault)));
            continue;
        }
        match mount.kind {
            MountKind::Path if !mount.source.starts_with('/') => {
                planned.left_out.push((
                    mount.name.clone(),
                    "its location is not an absolute path".to_owned(),
                ));
                continue;
            }
            MountKind::Path if !directory_exists(&mount.source) => {
                planned.left_out.push((
                    mount.name.clone(),
                    format!("the directory {} is not on this host", mount.source),
                ));
                continue;
            }
            MountKind::Volume if !is_volume_name(&mount.source) => {
                planned.left_out.push((
                    mount.name.clone(),
                    "its location is not a volume name".to_owned(),
                ));
                continue;
            }
            _ => {}
        }
        let mut mode = attach_mode(mount.mode, private_views);
        let view = (mode == MountMode::Private)
            .then(|| mount.layer.clone())
            .flatten();
        if mode == MountMode::Private && view.is_none() {
            // A view needs a place for its changes; without one the mount is read-only, never shared.
            tracing::warn!(mount = %mount.name, "a private view was asked for without a layer; attaching it read-only");
            mode = MountMode::ReadOnly;
        }
        if let Some(layer) = view {
            let overlay_ok = match mount.kind {
                MountKind::Path => is_overlay_safe(&mount.source),
                MountKind::Volume => true,
            };
            if overlay_ok {
                planned.views.push(View {
                    mount: mount.clone(),
                    layer,
                });
            } else {
                tracing::warn!(mount = %mount.name, "the location cannot be laid under a view; attaching it read-only");
                mode = MountMode::ReadOnly;
            }
        }
        if mode != MountMode::Private {
            let read_only = mode != MountMode::Shared;
            planned.direct.push(match mount.kind {
                MountKind::Path => bind(&mount.source, &mount.target, read_only),
                MountKind::Volume => volume(&mount.source, &mount.target, read_only),
            });
        }
        planned.attached.push(AttachedMount {
            name: mount.name.clone(),
            path: mount.target.clone(),
            mode,
        });
    }
    planned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(
        name: &str,
        kind: MountKind,
        source: &str,
        target: &str,
        mode: MountMode,
    ) -> MountSpec {
        MountSpec {
            name: name.into(),
            kind,
            source: source.into(),
            target: target.into(),
            mode,
            layer: (mode == MountMode::Private).then(|| MountLayer {
                name: format!("orch-over-{name}"),
                storage: None,
            }),
        }
    }

    const RESERVED: [&str; 3] = ["/workspace", "/home/agent", "/run/orch"];

    fn everything_exists(_: &str) -> bool {
        true
    }

    #[test]
    fn a_path_is_a_bind_mount_and_a_volume_a_named_volume_each_read_only_unless_shared() {
        let planned = plan(
            &[
                mount(
                    "src",
                    MountKind::Path,
                    "/srv/src",
                    "/data/src",
                    MountMode::ReadOnly,
                ),
                mount(
                    "cache",
                    MountKind::Volume,
                    "orch-mount-1",
                    "/mnt/cache",
                    MountMode::Shared,
                ),
            ],
            &RESERVED,
            true,
            &everything_exists,
        );
        assert_eq!(planned.direct.len(), 2);
        assert_eq!(planned.direct[0].typ, Some(MountType::BIND));
        assert_eq!(planned.direct[0].read_only, Some(true));
        assert_eq!(planned.direct[1].typ, Some(MountType::VOLUME));
        assert_eq!(planned.direct[1].read_only, Some(false));
        assert_eq!(
            planned.attached,
            vec![
                AttachedMount {
                    name: "src".into(),
                    path: "/data/src".into(),
                    mode: MountMode::ReadOnly
                },
                AttachedMount {
                    name: "cache".into(),
                    path: "/mnt/cache".into(),
                    mode: MountMode::Shared
                },
            ]
        );
        assert!(planned.views.is_empty() && planned.left_out.is_empty());
    }

    #[test]
    fn a_private_view_is_made_where_the_host_can_and_is_attached_read_only_where_it_cannot() {
        let asked = [mount(
            "src",
            MountKind::Path,
            "/srv/src",
            "/data/src",
            MountMode::Private,
        )];
        let can = plan(&asked, &RESERVED, true, &everything_exists);
        assert_eq!(can.views.len(), 1);
        assert!(
            can.direct.is_empty(),
            "the view becomes the mount when it is made"
        );
        assert_eq!(can.attached[0].mode, MountMode::Private);
        let cannot = plan(&asked, &RESERVED, false, &everything_exists);
        assert!(cannot.views.is_empty());
        assert_eq!(cannot.direct[0].read_only, Some(true));
        assert_eq!(cannot.attached[0].mode, MountMode::ReadOnly, "never shared");
    }

    #[test]
    fn a_view_whose_location_cannot_be_written_into_mount_options_is_read_only() {
        let planned = plan(
            &[mount(
                "odd",
                MountKind::Path,
                "/srv/a,b",
                "/data/odd",
                MountMode::Private,
            )],
            &RESERVED,
            true,
            &everything_exists,
        );
        assert!(planned.views.is_empty());
        assert_eq!(planned.attached[0].mode, MountMode::ReadOnly);
        assert!(
            is_overlay_safe("/srv/src") && !is_overlay_safe("/srv/a:b") && !is_overlay_safe("rel")
        );
    }

    #[test]
    fn a_mount_without_a_layer_is_never_a_view_and_never_shared() {
        let mut view = mount(
            "src",
            MountKind::Path,
            "/srv/src",
            "/data/src",
            MountMode::Private,
        );
        view.layer = None;
        let planned = plan(&[view], &RESERVED, true, &everything_exists);
        assert!(planned.views.is_empty());
        assert_eq!(planned.attached[0].mode, MountMode::ReadOnly);
    }

    #[test]
    fn mounts_that_cannot_be_attached_are_left_out_with_the_reason_and_the_nearer_one_stays() {
        let planned = plan(
            &[
                mount(
                    "near",
                    MountKind::Path,
                    "/srv/a",
                    "/data",
                    MountMode::ReadOnly,
                ),
                mount(
                    "far",
                    MountKind::Path,
                    "/srv/b",
                    "/data",
                    MountMode::ReadOnly,
                ),
                mount(
                    "work",
                    MountKind::Path,
                    "/srv/c",
                    "/workspace/cache",
                    MountMode::ReadOnly,
                ),
                mount(
                    "gone",
                    MountKind::Path,
                    "/srv/missing",
                    "/gone",
                    MountMode::ReadOnly,
                ),
                mount(
                    "rel",
                    MountKind::Path,
                    "srv/rel",
                    "/rel",
                    MountMode::ReadOnly,
                ),
                mount(
                    "vol",
                    MountKind::Volume,
                    "../x",
                    "/vol",
                    MountMode::ReadOnly,
                ),
            ],
            &RESERVED,
            true,
            &|path: &str| path != "/srv/missing",
        );
        let names: Vec<_> = planned.attached.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["near"]);
        let left: Vec<_> = planned.left_out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(left, ["far", "work", "gone", "rel", "vol"]);
        assert!(planned.left_out[2].1.contains("not on this host"));
    }
}
