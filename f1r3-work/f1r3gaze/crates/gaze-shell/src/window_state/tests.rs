use super::*;

fn monitor(name: &str, origin: (i32, i32), size: (u32, u32), scale: f64) -> Monitor {
    Monitor {
        name: Some(name.into()),
        uuid: None,
        origin,
        size,
        scale,
    }
}

fn saved_at(position: Option<(i32, i32)>, size: (f64, f64), units: Units, scale: f64, on: &Monitor) -> WindowState {
    WindowState {
        normal: Some(Normal {
            width: size.0,
            height: size.1,
            position,
            surface_offset: (0, 0),
            units,
            scale,
        }),
        monitor: Some(on.saved()),
        ..WindowState::default()
    }
}

#[test]
fn a_window_opens_where_it_was() {
    let left = monitor("DP-1", (0, 0), (2560, 1440), 1.0);
    let right = monitor("DP-2", (2560, 0), (1920, 1080), 1.0);
    let saved = saved_at(Some((2700, 100)), (1200.0, 800.0), Units::Physical, 1.0, &right);
    let plan = plan_restore(&saved, &[left, right], Some(0), WindowSystem::X11);
    assert_eq!(
        plan,
        RestorePlan {
            size: (1200.0, 800.0),
            position: Some((2700, 100)),
            maximized: false,
            fullscreen: None,
            zoom: 1.0,
        }
    );
}

#[test]
fn a_window_whose_monitor_is_gone_is_centred_on_the_primary() {
    let laptop = monitor("eDP-1", (0, 0), (1920, 1080), 1.0);
    let gone = monitor("DP-2", (1920, 0), (2560, 1440), 1.0);
    let saved = saved_at(Some((2100, 200)), (1200.0, 800.0), Units::Physical, 1.0, &gone);
    let plan = plan_restore(&saved, &[laptop], Some(0), WindowSystem::X11);
    assert_eq!(plan.position, Some(((1920 - 1200) / 2, (1080 - 800) / 2)));
}

#[test]
fn a_mostly_off_screen_window_is_brought_back() {
    let m = monitor("DP-1", (0, 0), (1920, 1080), 1.0);
    let place = |x: i32| {
        let saved = saved_at(Some((x, 100)), (800.0, 600.0), Units::Physical, 1.0, &m);
        plan_restore(&saved, std::slice::from_ref(&m), Some(0), WindowSystem::Windows).position
    };
    let centred = Some(((1920 - 800) / 2, (1080 - 600) / 2));
    assert_eq!(place(1920 - 96), Some((1920 - 96, 100)), "96 px over the monitor is enough");
    assert_eq!(place(1920 - 95), centred, "95 px is not");
    assert_eq!(place(-800 + 96), Some((-800 + 96, 100)));
    assert_eq!(place(-800 + 95), centred);
}

#[test]
fn the_title_bar_must_be_reachable() {
    // Physical pixels at scale 2: σ = 2, so the top edge may be from
    // 0 − 16·2 = −32 to 1080 − 48·2 = 984.
    let m = monitor("DP-1", (0, 0), (1920, 1080), 2.0);
    let at = |y: i32| {
        let saved = saved_at(Some((100, y)), (400.0, 300.0), Units::Physical, 2.0, &m);
        plan_restore(&saved, std::slice::from_ref(&m), Some(0), WindowSystem::X11).position
    };
    let centred = Some(((1920 - 800) / 2, (1080 - 600) / 2));
    assert_eq!(at(-32), Some((100, -32)));
    assert_eq!(at(984), Some((100, 984)));
    assert_eq!(at(-33), centred);
    assert_eq!(at(985), centred);
}

#[test]
fn a_window_larger_than_its_monitor_shrinks_to_fit() {
    let small = monitor("LVDS-1", (0, 0), (1366, 768), 1.0);
    let saved = saved_at(Some((0, 0)), (1600.0, 1000.0), Units::Physical, 1.0, &small);
    assert_eq!(plan_restore(&saved, std::slice::from_ref(&small), Some(0), WindowSystem::X11).size, (1366.0, 768.0));
    // After a scaling change to 1.5 the monitor is 910.67 × 512 logical.
    let scaled = monitor("LVDS-1", (0, 0), (1366, 768), 1.5);
    let size = plan_restore(&saved, &[scaled], Some(0), WindowSystem::X11).size;
    assert!((size.0 - 1366.0 / 1.5).abs() < 1e-9 && (size.1 - 512.0).abs() < 1e-9, "{size:?}");
    // Never smaller than the minimum, unless the monitor is.
    let tiny = saved_at(Some((0, 0)), (10.0, 10.0), Units::Physical, 1.0, &small);
    assert_eq!(plan_restore(&tiny, std::slice::from_ref(&small), Some(0), WindowSystem::X11).size, MIN_SIZE);
}

#[test]
fn on_macos_the_surface_is_placed() {
    // Points: σ = 1 whatever the scale. The frame was at (100, 50) with its
    // surface 28 points lower, below the title bar.
    let display = Monitor {
        name: Some("Built-in Retina Display".into()),
        uuid: Some("37D8832A-2D66-02CA-B9F7-8F30A301B230".into()),
        origin: (0, 0),
        size: (1440, 900),
        scale: 2.0,
    };
    let mut saved = saved_at(Some((100, 50)), (1000.0, 700.0), Units::Points, 2.0, &display);
    if let Some(normal) = &mut saved.normal {
        normal.surface_offset = (0, 28);
    }
    let plan = plan_restore(&saved, std::slice::from_ref(&display), Some(0), WindowSystem::MacOs);
    assert_eq!(plan.position, Some((100, 78)));
}

#[test]
fn wayland_never_positions_a_window() {
    let m = monitor("eDP-1", (0, 0), (2880, 1800), 2.0);
    let saved = saved_at(Some((10, 10)), (2000.0, 1000.0), Units::Physical, 1.0, &m);
    let plan = plan_restore(&saved, std::slice::from_ref(&m), Some(0), WindowSystem::Wayland);
    assert_eq!(plan.position, None);
    // Was: shrunk to the output's mode divided by its scale, (1440, 900).
    // winit-wayland's monitor scale is the whole-number wl_output scale, not
    // the fractional one, and the compositor bounds a new window itself
    // (configure_bounds), so the size is kept (ledger S13, part 2).
    // assert_eq!(plan.size, (1440.0, 900.0), "shrunk to the monitor's logical size");
    assert_eq!(plan.size, (2000.0, 1000.0), "the compositor bounds the size");
}

#[test]
fn positions_in_other_units_are_not_used() {
    // A profile carried from macOS to X11: its points mean nothing here.
    let m = monitor("DP-1", (0, 0), (1920, 1080), 1.0);
    let saved = saved_at(Some((100, 100)), (800.0, 600.0), Units::Points, 2.0, &m);
    assert_eq!(plan_restore(&saved, std::slice::from_ref(&m), Some(0), WindowSystem::X11).position, None);
}

#[test]
fn monitors_are_found_by_uuid_then_name_then_origin_then_size() {
    let with_uuid = |uuid: &str, name: &str, origin: (i32, i32)| Monitor {
        uuid: Some(uuid.into()),
        ..monitor(name, origin, (1920, 1080), 1.0)
    };
    let saved = SavedMonitor {
        name: Some("Built-in".into()),
        uuid: Some("A".into()),
        origin: (0, 0),
        size: (1920, 1080),
        scale: 1.0,
    };
    let monitors = [with_uuid("B", "Built-in", (0, 0)), with_uuid("A", "Other", (1920, 0))];
    assert_eq!(find_monitor(&saved, &monitors), Some(1), "the UUID first");
    let twins = [monitor("DELL U2720Q", (0, 0), (3840, 2160), 1.0), monitor("DELL U2720Q", (3840, 0), (3840, 2160), 1.0)];
    let right_twin = SavedMonitor {
        name: Some("DELL U2720Q".into()),
        origin: (3840, 0),
        ..SavedMonitor::default()
    };
    assert_eq!(find_monitor(&right_twin, &twins), Some(1), "two of one name: the one at the same origin");
    let renamed = SavedMonitor {
        name: Some("gone".into()),
        origin: (3840, 0),
        size: (1, 1),
        ..SavedMonitor::default()
    };
    assert_eq!(find_monitor(&renamed, &twins), Some(1), "then the origin");
    let moved = SavedMonitor {
        origin: (7, 7),
        size: (3840, 2160),
        ..SavedMonitor::default()
    };
    assert_eq!(find_monitor(&moved, &twins), Some(0), "then the size");
    assert_eq!(find_monitor(&SavedMonitor { size: (1, 1), origin: (5, 5), ..SavedMonitor::default() }, &twins), None);
}

#[test]
fn full_screen_returns_to_its_monitor_or_the_current_one() {
    let a = monitor("A", (0, 0), (1920, 1080), 1.0);
    let b = monitor("B", (1920, 0), (1920, 1080), 1.0);
    let mut saved = saved_at(Some((2000, 100)), (800.0, 600.0), Units::Physical, 1.0, &b);
    saved.fullscreen = true;
    saved.maximized = true;
    let plan = plan_restore(&saved, &[a.clone(), b], Some(0), WindowSystem::Windows);
    assert_eq!((plan.fullscreen, plan.maximized), (Some(FullScreen::On(1)), true));
    // A monitor of the same size is taken for the saved one (the last rule
    // of find_monitor), as when placing the window.
    assert_eq!(plan_restore(&saved, std::slice::from_ref(&a), Some(0), WindowSystem::Windows).fullscreen, Some(FullScreen::On(0)));
    let other = monitor("C", (0, 0), (2560, 1440), 1.0);
    assert_eq!(plan_restore(&saved, &[other], Some(0), WindowSystem::Windows).fullscreen, Some(FullScreen::Current));
}

fn sample(width: f64, height: f64, position: Option<(i32, i32)>) -> Sample {
    Sample {
        width,
        height,
        position,
        surface_position: position.map(|(x, y)| (x, y + 30)),
        units: Units::Physical,
        scale: 1.0,
        maximized: false,
        fullscreen: false,
        minimized: false,
        monitor: Some(monitor("DP-1", (0, 0), (1920, 1080), 1.0)),
    }
}

#[test]
fn maximizing_keeps_the_normal_size_and_place() {
    let mut tracker = Tracker::new(WindowState::default());
    assert!(tracker.observe(&sample(1000.0, 700.0, Some((10, 20)))));
    let normal = tracker.state().normal.clone().expect("normal");
    assert_eq!((normal.width, normal.position, normal.surface_offset), (1000.0, Some((10, 20)), (0, 30)));
    let maximized = Sample {
        maximized: true,
        ..sample(1920.0, 1050.0, Some((0, 0)))
    };
    assert!(tracker.observe(&maximized));
    assert!(tracker.state().maximized);
    assert_eq!(tracker.state().normal.as_ref(), Some(&normal), "the size before maximizing is kept");
    let full = Sample {
        fullscreen: true,
        ..sample(1920.0, 1080.0, Some((0, 0)))
    };
    assert!(tracker.observe(&full));
    assert_eq!(tracker.state().normal.as_ref(), Some(&normal));
    assert!(!tracker.observe(&full), "the same sample changes nothing");
}

#[test]
fn minimized_and_degenerate_samples_change_nothing() {
    let mut tracker = Tracker::new(WindowState::default());
    tracker.observe(&sample(1000.0, 700.0, Some((10, 20))));
    let before = tracker.state().clone();
    let ignored = [
        Sample { minimized: true, ..sample(10.0, 10.0, Some((0, 0))) },
        sample(1000.0, 700.0, Some((-32_000, -32_000))),
        sample(0.0, 700.0, Some((10, 20))),
        sample(f64::NAN, 700.0, Some((10, 20))),
        sample(1000.0, f64::INFINITY, Some((10, 20))),
    ];
    for s in &ignored {
        assert!(!tracker.observe(s), "{s:?}");
    }
    assert_eq!(tracker.state(), &before);
}

#[test]
fn the_zoom_is_kept_in_range() {
    let mut tracker = Tracker::new(WindowState::default());
    assert!(tracker.set_zoom(9.0));
    assert_eq!(tracker.state().zoom, 5.0);
    assert!(tracker.set_zoom(-1.0));
    assert_eq!(tracker.state().zoom, 0.25);
    assert!(tracker.set_zoom(f64::NAN));
    assert_eq!(tracker.state().zoom, 1.0);
    assert!(!tracker.set_zoom(1.0));
    assert_eq!(WindowState::read(br#"{"zoom": 12.5}"#).expect("read").zoom, 5.0);
    // Blitz keeps the zoom in an f32, which holds 1.2 as 1.2000000476837158:
    // kept to hundredths, as its keys step in tenths.
    assert!(tracker.set_zoom(f64::from(1.2_f32)));
    assert_eq!(tracker.state().zoom, 1.2);
}

#[test]
fn the_save_clock_waits_for_quiet_but_not_forever() {
    let t0 = Instant::now();
    let ms = |n: u64| t0 + Duration::from_millis(n);
    let mut clock = SaveClock::default();
    assert_eq!(clock.due(), None);
    clock.changed(ms(0));
    assert_eq!(clock.due(), Some(ms(500)));
    clock.changed(ms(400));
    assert_eq!(clock.due(), Some(ms(900)));
    // A long drag: a change every 100 ms is never quiet for 500 ms.
    for n in 5..=35 {
        clock.changed(ms(n * 100));
    }
    assert_eq!(clock.due(), Some(ms(3_000)), "at least every three seconds");
    clock.saved();
    assert_eq!(clock.due(), None);
}

#[test]
fn window_files_tolerate_new_fields_and_refuse_damage() {
    let m = monitor("DP-1", (0, 0), (1920, 1080), 1.25);
    let state = WindowState {
        maximized: true,
        zoom: 1.5,
        ..saved_at(Some((-5, 7)), (1024.0, 640.0), Units::Physical, 1.25, &m)
    };
    assert_eq!(WindowState::read(&state.to_bytes()), Ok(state));
    let later = br#"{"normal": {"width": 800, "height": 600, "units": "physical", "scale": 1, "snapped": "left"}, "tabs_strip": "top"}"#;
    assert_eq!(WindowState::read(later).expect("unknown fields are ignored").normal.map(|n| n.width), Some(800.0));
    assert!(matches!(WindowState::read(br#"{"zoom": "big"}"#), Err(StateError::Corrupt(_))));
    assert!(matches!(WindowState::read(br#"{"normal": {"width": 800}}"#), Err(StateError::Corrupt(_))));
    assert_eq!(WindowState::read(br#"{"version": 2}"#), Err(StateError::Newer { version: 2, known: WINDOW_VERSION }));
    let negative = br#"{"normal": {"width": -800, "height": 600, "units": "physical", "scale": 0}}"#;
    assert_eq!(WindowState::read(negative).expect("read").normal, None, "a size that is not positive is dropped");
}

#[test]
fn extreme_values_never_overflow() {
    let edges = [i32::MIN, -32_001, -1, 0, 1, i32::MAX];
    let sizes = [0u32, 1, 1920, u32::MAX];
    for &x in &edges {
        for &y in &edges {
            for &w in &sizes {
                let m = monitor("M", (x, y), (w, w), 1.0);
                for system in [WindowSystem::X11, WindowSystem::MacOs, WindowSystem::Windows, WindowSystem::Wayland] {
                    for units in [Units::Physical, Units::Points] {
                        let saved = saved_at(Some((y, x)), (f64::from(w).max(1.0), 1e12), units, 1e-300, &m);
                        let plan = plan_restore(&saved, std::slice::from_ref(&m), Some(0), system);
                        assert!(plan.size.0.is_finite() && plan.size.1.is_finite(), "{plan:?}");
                    }
                }
            }
        }
    }
}

// ── Step 11 (ledger S13, part 2): keeping window.json ───────────────────

fn saved_normal() -> WindowState {
    let m = monitor("DP-1", (0, 0), (1920, 1080), 1.0);
    saved_at(Some((10, 20)), (1000.0, 700.0), Units::Physical, 1.0, &m)
}

/// The sample a window made from `saved_normal` gives.
fn as_saved() -> Sample {
    Sample {
        surface_position: Some((10, 20)),
        ..sample(1000.0, 700.0, Some((10, 20)))
    }
}

#[test]
fn only_changes_are_written() {
    let mut keeper = Keeper::new(saved_normal());
    let mut writes = Vec::new();
    assert!(!keeper.observe(&as_saved()), "the window as it was saved");
    assert!(!keeper.save_with(|s| { writes.push(s.clone()); Ok(()) }), "nothing changed, nothing is written");
    let wider = Sample { width: 1100.0, ..as_saved() };
    assert!(keeper.observe(&wider));
    assert!(keeper.save_with(|s| { writes.push(s.clone()); Ok(()) }));
    assert!(!keeper.save_with(|s| { writes.push(s.clone()); Ok(()) }), "written once");
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].normal.as_ref().map(|n| n.width), Some(1100.0));
    let t0 = Instant::now();
    keeper.changed(t0);
    assert_eq!(keeper.due(), Some(t0 + SaveClock::QUIET));
    keeper.save_with(|_| Ok(()));
    assert_eq!(keeper.due(), None, "a save starts the clock over");
}

#[test]
fn a_failed_write_is_tried_again() {
    let mut keeper = Keeper::new(saved_normal());
    keeper.observe(&Sample { width: 1100.0, ..as_saved() });
    assert!(keeper.save_with(|_| Err("read-only".into())));
    let mut wrote = None;
    assert!(keeper.save_with(|s| { wrote = Some(s.clone()); Ok(()) }), "the next save writes it");
    assert_eq!(wrote.and_then(|s| s.normal).map(|n| n.width), Some(1100.0));
    assert!(!keeper.save_with(|_| Ok(())));
}

#[test]
fn a_zoom_change_is_saved_when_due() {
    let mut keeper = Keeper::new(WindowState::default());
    let t0 = Instant::now();
    assert!(keeper.set_zoom(1.25, t0));
    assert_eq!(keeper.due(), Some(t0 + SaveClock::QUIET));
    let t1 = t0 + Duration::from_millis(100);
    assert!(!keeper.set_zoom(f64::from(1.25_f32), t1), "the same zoom, from Blitz's f32");
    assert_eq!(keeper.due(), Some(t0 + SaveClock::QUIET), "an unchanged zoom delays nothing");
    let mut wrote = None;
    assert!(keeper.save_with(|s| { wrote = Some(s.zoom); Ok(()) }));
    assert_eq!(wrote, Some(1.25));
}

#[test]
fn samples_are_read_in_each_platforms_units() {
    let reading = |surface_size, scale, frame, inset, minimized| WindowReading {
        surface_size,
        scale,
        frame,
        inset,
        maximized: false,
        fullscreen: false,
        minimized,
        monitor: None,
    };
    // macOS: physical pixels divided by the window's scale are points.
    let mac = Sample::from_platform(reading((2000, 1400), 2.0, Some((200, 100)), (0, 56), Some(false)), Some(Units::Points));
    assert_eq!((mac.width, mac.height), (1000.0, 700.0));
    assert_eq!((mac.position, mac.surface_position, mac.units), (Some((100, 50)), Some((100, 78)), Units::Points));
    // X11: physical pixels as they are.
    let x11 = Sample::from_platform(reading((1000, 700), 1.0, Some((10, 20)), (1, 22), Some(false)), Some(Units::Physical));
    assert_eq!((x11.position, x11.surface_position, x11.units), (Some((10, 20)), Some((11, 42)), Units::Physical));
    // Wayland: no position, and a window that cannot say is not minimized.
    let wayland = Sample::from_platform(reading((2000, 1000), 2.0, None, (0, 0), None), None);
    assert_eq!((wayland.width, wayland.position, wayland.surface_position, wayland.minimized), (1000.0, None, None, false));
    // Windows: a minimized window's (−32000, −32000) changes nothing.
    let mut tracker = Tracker::new(saved_normal());
    let before = tracker.state().clone();
    let minimized = Sample::from_platform(reading((160, 28), 1.0, Some((-32_000, -32_000)), (8, 31), Some(false)), Some(Units::Physical));
    assert!(!tracker.observe(&minimized));
    assert_eq!(tracker.state(), &before);
}

#[test]
fn monitors_are_described_in_desktop_units() {
    let reading = MonitorReading {
        name: Some("Monitor #2".into()),
        uuid: Some("37d8832a2d6602cab9f78f30a301b230".into()),
        position: (2880, 0),
        mode: (5120, 2880),
        scale: 2.0,
    };
    let mac = Monitor::from_platform(reading.clone(), Some(Units::Points));
    assert_eq!((mac.origin, mac.size, mac.scale), ((1440, 0), (2560, 1440), 2.0));
    for units in [Some(Units::Physical), None] {
        let kept = Monitor::from_platform(reading.clone(), units);
        assert_eq!((kept.origin, kept.size), ((2880, 0), (5120, 2880)), "{units:?}");
    }
}

#[test]
fn wayland_leaves_the_size_to_the_compositor() {
    let m = monitor("eDP-1", (0, 0), (2880, 1800), 2.0);
    let size = |w: f64, h: f64| {
        let saved = saved_at(None, (w, h), Units::Physical, 1.0, &m);
        plan_restore(&saved, std::slice::from_ref(&m), Some(0), WindowSystem::Wayland).size
    };
    assert_eq!(size(2000.0, 1000.0), (2000.0, 1000.0));
    assert_eq!(size(9000.0, 9000.0), MAX_SIZE);
    assert_eq!(size(10.0, 10.0), MIN_SIZE);
}

#[test]
fn sizes_without_a_monitor_are_capped() {
    let m = monitor("gone", (0, 0), (1920, 1080), 1.0);
    let saved = saved_at(Some((0, 0)), (1e9, 1e9), Units::Physical, 1.0, &m);
    for system in [WindowSystem::X11, WindowSystem::MacOs, WindowSystem::Windows, WindowSystem::Wayland] {
        assert_eq!(plan_restore(&saved, &[], None, system).size, MAX_SIZE, "{system:?}");
    }
}

#[test]
fn zoom_keys_stay_within_range() {
    assert_eq!(zoom_correction(0.15), Some(0.25));
    assert_eq!(zoom_correction(-0.2), Some(0.25));
    assert_eq!(zoom_correction(5.05), Some(5.0));
    assert_eq!(zoom_correction(f64::NAN), Some(1.0));
    for kept in [1.2, f64::from(1.2_f32), 0.25, 5.0] {
        assert_eq!(zoom_correction(kept), None, "{kept}");
    }
}
