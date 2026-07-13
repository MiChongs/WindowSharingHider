#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer,
    TimerMode, VecModel,
};
use window_sharing_hider::app::{AppController, AppView, StatusTone, WindowRowView};
use window_sharing_hider::model::WindowIcon;
use window_sharing_hider::platform::windows::WindowsPlatform;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    let ui = AppWindow::new()?;
    let controller = Rc::new(RefCell::new(AppController::new(Arc::new(WindowsPlatform))?));
    let rows = Rc::new(VecModel::<WindowRow>::default());
    ui.set_window_rows(ModelRc::from(Rc::clone(&rows)));

    bind_callbacks(&ui, Rc::clone(&controller), Rc::clone(&rows));

    controller.borrow_mut().request_refresh();
    sync_ui(&ui, &rows, controller.borrow().view());

    let event_timer = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let controller = Rc::clone(&controller);
        let rows = Rc::clone(&rows);
        event_timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let changed = controller.borrow_mut().drain_worker_events();
            if changed {
                let view = controller.borrow().view();
                sync_ui(&ui, &rows, view);
            }
        });
    }

    let refresh_timer = Timer::default();
    {
        let ui_weak = ui.as_weak();
        let controller = Rc::clone(&controller);
        let rows = Rc::clone(&rows);
        refresh_timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            controller.borrow_mut().request_refresh();
            let view = controller.borrow().view();
            sync_ui(&ui, &rows, view);
        });
    }

    ui.run()?;
    event_timer.stop();
    refresh_timer.stop();
    controller.borrow_mut().shutdown();
    Ok(())
}

fn bind_callbacks(
    ui: &AppWindow,
    controller: Rc<RefCell<AppController>>,
    rows: Rc<VecModel<WindowRow>>,
) {
    {
        let ui_weak = ui.as_weak();
        let controller = Rc::clone(&controller);
        let rows = Rc::clone(&rows);
        ui.on_refresh_requested(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            controller.borrow_mut().request_refresh();
            sync_ui(&ui, &rows, controller.borrow().view());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let controller = Rc::clone(&controller);
        let rows = Rc::clone(&rows);
        ui.on_window_toggled(move |row_id, enabled| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            controller.borrow_mut().toggle_window(row_id, enabled);
            sync_ui(&ui, &rows, controller.borrow().view());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let controller = Rc::clone(&controller);
        let rows = Rc::clone(&rows);
        ui.on_system_mode_toggled(move |enabled| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            controller.borrow_mut().set_system_mode(enabled);
            sync_ui(&ui, &rows, controller.borrow().view());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let controller = Rc::clone(&controller);
        ui.on_wetype_toggled(move |enabled| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            controller.borrow_mut().set_wetype_enabled(enabled);
            sync_ui(&ui, &rows, controller.borrow().view());
        });
    }
}

fn sync_ui(ui: &AppWindow, model: &Rc<VecModel<WindowRow>>, view: AppView) {
    let same_identity_order = model.row_count() == view.rows.len()
        && view.rows.iter().enumerate().all(|(index, next)| {
            model
                .row_data(index)
                .is_some_and(|current| current.row_id == next.row_id)
        });

    if same_identity_order {
        for (index, row) in view.rows.into_iter().enumerate() {
            let current = model.row_data(index);
            let next = to_slint_row(row, current.as_ref());
            if current.as_ref() != Some(&next) {
                model.set_row_data(index, next);
            }
        }
    } else {
        model.set_vec(
            view.rows
                .into_iter()
                .map(|row| to_slint_row(row, None))
                .collect::<Vec<_>>(),
        );
    }

    ui.set_status_message(SharedString::from(view.status_message));
    ui.set_status_tone(tone_value(view.status_tone));
    ui.set_scanning(view.scanning);
    ui.set_system_mode(view.system_mode);
    ui.set_wetype_enabled(view.wetype_enabled);
    ui.set_wetype_found(view.wetype_found);
    ui.set_protected_count(i32::try_from(view.protected_count).unwrap_or(i32::MAX));
}

fn to_slint_row(row: WindowRowView, current: Option<&WindowRow>) -> WindowRow {
    let (icon, icon_key, has_icon) = match row.icon.as_ref() {
        Some(process_icon) => {
            let cached = current
                .filter(|current| {
                    current.has_icon && current.icon_key.as_str() == process_icon.source()
                })
                .map(|current| current.icon.clone());
            (
                cached.unwrap_or_else(|| to_slint_image(process_icon)),
                SharedString::from(process_icon.source()),
                true,
            )
        }
        None => (Image::default(), SharedString::default(), false),
    };

    WindowRow {
        row_id: row.row_id,
        title: row.title.into(),
        detail: row.detail.into(),
        icon,
        icon_key,
        has_icon,
        status: row.status.into(),
        status_tone: tone_value(row.status_tone),
        enabled: row.enabled,
        busy: row.busy,
        system_candidate: row.system_candidate,
    }
}

fn to_slint_image(icon: &WindowIcon) -> Image {
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(icon.width(), icon.height());
    pixels.make_mut_bytes().copy_from_slice(icon.rgba());
    Image::from_rgba8(pixels)
}

const fn tone_value(tone: StatusTone) -> i32 {
    match tone {
        StatusTone::Neutral => 0,
        StatusTone::Active => 1,
        StatusTone::Success => 2,
        StatusTone::Error => 3,
    }
}
