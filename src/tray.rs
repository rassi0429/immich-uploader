use anyhow::Result;
use tray_icon::TrayIcon;
use tray_icon::TrayIconBuilder;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};

pub struct TrayState {
    #[allow(dead_code)]
    pub icon: TrayIcon,
    pub menu_show_id: MenuId,
    pub menu_sync_all_id: MenuId,
    pub menu_pause_all_id: MenuId,
    pub menu_resume_all_id: MenuId,
    pub menu_quit_id: MenuId,
}

pub fn build_tray() -> Result<TrayState> {
    let icon = build_icon()?;
    let menu = Menu::new();
    let show = MenuItem::new("ウィンドウを表示", true, None);
    let sync_all = MenuItem::new("全フォルダ Sync Now", true, None);
    let pause_all = MenuItem::new("全フォルダ 一時停止", true, None);
    let resume_all = MenuItem::new("全フォルダ 再開", true, None);
    let quit = MenuItem::new("終了", true, None);

    menu.append(&show)?;
    menu.append(&sync_all)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&pause_all)?;
    menu.append(&resume_all)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;

    let menu_show_id = show.id().clone();
    let menu_sync_all_id = sync_all.id().clone();
    let menu_pause_all_id = pause_all.id().clone();
    let menu_resume_all_id = resume_all.id().clone();
    let menu_quit_id = quit.id().clone();

    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Immich Auto Uploader")
        .with_icon(icon)
        .build()?;

    Ok(TrayState {
        icon,
        menu_show_id,
        menu_sync_all_id,
        menu_pause_all_id,
        menu_resume_all_id,
        menu_quit_id,
    })
}

fn build_icon() -> Result<tray_icon::Icon> {
    let size: u32 = 32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let center = size as f32 / 2.0;
    let radius = size as f32 / 2.2;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= radius {
                if d <= radius - 6.0 {
                    data.extend_from_slice(&[36, 124, 204, 255]);
                } else {
                    data.extend_from_slice(&[255, 255, 255, 255]);
                }
            } else {
                data.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Ok(tray_icon::Icon::from_rgba(data, size, size)?)
}
