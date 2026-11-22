// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn direct3d_12_alone_is_tried_first_only_on_windows_when_nothing_was_chosen() {
    use super::{RendererChoice, dx12_first};
    assert!(dx12_first(RendererChoice::Auto, true, false));
    assert!(!dx12_first(RendererChoice::Auto, false, false), "only Windows");
    assert!(!dx12_first(RendererChoice::Auto, true, true), "WGPU_BACKEND or WGPU_POWER_PREF decides instead");
    assert!(!dx12_first(RendererChoice::Wgpu, true, false), "PDFCRAFT_RENDERER=wgpu keeps every backend, in one run");
    assert!(!dx12_first(RendererChoice::Gl, true, false));
}

#[test]
fn the_direct3d_12_run_is_narrowed_only_where_displays_pick_the_adapter() {
    use eframe::wgpu;
    // Without a displays-based selector (no display information), nothing is narrowed.
    let mut native = super::native_options(false, false, eframe::Renderer::Wgpu);
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut native.wgpu_options.wgpu_setup else {
        panic!("default setup creates its own instance")
    };
    setup.native_adapter_selector = None;
    let before = setup.instance_descriptor.backends;
    assert!(!super::hardware_dx12_only(&mut native, vec![panel(NVIDIA)]));
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &native.wgpu_options.wgpu_setup else { panic!("default setup creates its own instance") };
    assert_eq!(setup.instance_descriptor.backends, before);
    // With one, the run lists Direct3D 12 alone.
    let mut native = super::native_options(false, false, eframe::Renderer::Wgpu);
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut native.wgpu_options.wgpu_setup else {
        panic!("default setup creates its own instance")
    };
    setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters: &[wgpu::Adapter], _: Option<&wgpu::Surface<'_>>| {
        adapters.first().cloned().ok_or_else(|| "none".to_string())
    }));
    assert!(super::hardware_dx12_only(&mut native, vec![panel(NVIDIA)]));
    let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &native.wgpu_options.wgpu_setup else { panic!("default setup creates its own instance") };
    assert_eq!(setup.instance_descriptor.backends, wgpu::Backends::DX12);
}

#[test]
fn the_direct3d_12_run_needs_somewhere_to_remember_a_failure() {
    let dir = scratch();
    let file = dir.join("settings").join(super::DX12_ALONE_FAILED);
    assert!(super::can_remember(&file), "creates the folder and an empty file");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
    std::fs::write(&file, "10de:2684").unwrap();
    assert!(super::can_remember(&file));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "10de:2684", "an existing record is left as it is");
    // A folder that can't be made (here a file stands in its place) can't hold the record.
    let blocked = dir.join("settings").join(super::DX12_ALONE_FAILED).join(super::DX12_ALONE_FAILED);
    assert!(!super::can_remember(&blocked));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_direct3d_12_run_is_skipped_until_the_display_gpus_change() {
    use super::{displays_key, dx12_alone_failed_here as failed_here};
    let displays = [panel(INTEL_IGPU), monitor(NVIDIA)];
    let saved = displays_key(&displays);
    assert_eq!(saved, "10de:2684,8086:a780", "sorted, so the display order doesn't matter");
    assert_eq!(displays_key(&[monitor(NVIDIA), panel(INTEL_IGPU)]), saved);
    assert!(failed_here(Some(&format!("{saved}\n")), &displays));
    assert!(!failed_here(None, &displays), "nothing failed yet");
    assert!(!failed_here(Some(&saved), &[panel(INTEL_IGPU)]), "another GPU set tries again");
    assert!(!failed_here(Some(""), &displays));
}

#[test]
fn the_direct3d_12_run_picks_what_the_combined_run_would() {
    use super::{dx12_alone_may_use, pick_adapter};
    use eframe::wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other};
    const WARP: (u32, u32, eframe::wgpu::DeviceType) = (0x1414, 0x8c, Cpu);
    // wgpu lists Direct3D 12 adapters before OpenGL ones, which carry no PCI device id.
    let gl = [(NVIDIA.0, 0, Other), (INTEL_IGPU.0, 0, IntegratedGpu)];
    let both_dx12 = vec![(NVIDIA.0, NVIDIA.1, DiscreteGpu), (INTEL_IGPU.0, INTEL_IGPU.1, IntegratedGpu), WARP];
    // A hybrid laptop whose Intel GPU has only an OpenGL driver.
    let nvidia_dx12 = vec![(NVIDIA.0, NVIDIA.1, DiscreteGpu), WARP];
    let layouts = [
        vec![monitor(NVIDIA)],
        vec![panel(INTEL_IGPU)],
        vec![panel(INTEL_IGPU), monitor(NVIDIA)],
        vec![monitor(INTEL_IGPU), panel(NVIDIA)],
        vec![monitor(INTEL_IGPU), monitor(NVIDIA)],
    ];
    for dx12 in [&both_dx12, &nvidia_dx12] {
        let combined: Vec<_> = dx12.iter().chain(&gl).copied().collect();
        for displays in &layouts {
            let alone: Vec<_> = dx12.iter().copied().filter(|&(v, d, kind)| dx12_alone_may_use((v, d), kind, displays)).collect();
            // Where the Direct3D 12 run picks an adapter, the combined run picks the same one;
            // where it picks none, it fails and the combined run decides.
            if let Some(i) = pick_adapter(&alone, displays, None) {
                assert_eq!(pick_adapter(&combined, displays, None).map(|j| combined[j]), Some(alone[i]), "{dx12:?} {displays:?}");
            }
        }
    }
    let alone: Vec<_> = nvidia_dx12.iter().copied().filter(|&(v, d, kind)| dx12_alone_may_use((v, d), kind, &[panel(INTEL_IGPU)])).collect();
    assert!(alone.is_empty(), "issue #8: the laptop keeps drawing on its panel's GPU through OpenGL");
}

#[test]
fn the_direct3d_12_run_draws_only_on_a_gpu_driving_a_display() {
    use super::dx12_alone_may_use as may_use;
    use eframe::wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu};
    // The GPU behind the monitor, integrated or discrete.
    assert!(may_use(NVIDIA, DiscreteGpu, &[monitor(NVIDIA)]));
    assert!(may_use(INTEL_IGPU, IntegratedGpu, &[panel(INTEL_IGPU), monitor(NVIDIA)]));
    // Issue #8: a hybrid laptop whose panel GPU has only an OpenGL driver. Its discrete GPU has
    // Direct3D 12 but drives nothing, so the run lists nothing and the next run picks the panel's
    // GPU through OpenGL, as it did before.
    assert!(!may_use(NVIDIA, DiscreteGpu, &[panel(INTEL_IGPU)]));
    // Software (WARP), even with ids that match a display, and any GPU when no display is known.
    assert!(!may_use(NVIDIA, Cpu, &[monitor(NVIDIA)]));
    assert!(!may_use((0x1414, 0x8c), Cpu, &[]));
    assert!(!may_use(NVIDIA, DiscreteGpu, &[]));
}

/// Inside the Flatpak the window names the exported desktop entry; everywhere else, and in
/// another app's sandbox, it keeps the id the other packages install.
#[test]
fn the_window_names_the_flatpak_desktop_entry_only_in_its_own_sandbox() {
    assert_eq!(super::window_app_id(Some(super::FLATPAK_APP_ID)), super::FLATPAK_APP_ID);
    assert_eq!(super::window_app_id(None), super::APP_ID);
    assert_eq!(super::window_app_id(Some("org.example.Other")), super::APP_ID);
}

/// Both manifests rewrite `StartupWMClass` to the id the window carries in the sandbox.
#[test]
fn the_flatpak_desktop_entries_name_the_window_class() {
    for manifest in [
        include_str!("../../../packaging/linux/flatpak/io.github.yungSNAT.PlutarchPages.yml"),
        include_str!("../../../packaging/linux/flatpak/io.github.yungSNAT.PlutarchPages.bundle.yml"),
    ] {
        assert!(manifest.contains(&format!("s/^StartupWMClass=.*/StartupWMClass={}/", super::FLATPAK_APP_ID)));
    }
}
