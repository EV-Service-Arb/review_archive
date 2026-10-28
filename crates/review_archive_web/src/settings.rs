//! The member's settings. Every one is a [`Choice`]; resetting puts them all back on their
//! defaults.

use dioxus::prelude::*;
use ev_lib::uikit_dioxus::{Button, ButtonVariant, Card, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Size};
use review_archive_client::dto::{Setting, SettingsPatch};

use crate::{Api, Refresh, act, shown};

const INTERVALS: [u64; 6] = [3600, 6 * 3600, 12 * 3600, 86_400, 2 * 86_400, 7 * 86_400];

#[component]
pub fn Page() -> Element {
	let Api(api) = use_context();
	let Refresh(refresh) = use_context();
	let settings = use_resource(move || {
		let api = api.clone();
		async move {
			refresh();
			api.settings().await.map_err(shown)
		}
	});
	let settings = match &*settings.read() {
		Some(Ok(s)) => s.clone(),
		Some(Err(e)) => return rsx! { div { class: "p-6 text-accent-error", "{e}" } },
		None => return rsx! { div { class: "p-6 text-ink-soft", "Loading…" } },
	};
	rsx! {
		header { class: "flex h-14 shrink-0 items-center border-b border-border px-6",
			span { class: "font-medium", "Settings" }
		}
		main { class: "min-h-0 flex-1 overflow-y-auto p-6",
			Card { class: "max-w-xl gap-0 rounded-lg py-0",
				Choice {
					label: "Scan every",
					hint: "Every place under every gmail; a place another member tracks too goes by whichever of you wants it sooner. Each scan costs tokens.",
					options: INTERVALS.to_vec(),
					setting: settings.interval_secs,
					on_choose: move |name: String| act(move |c| async move { c.patch_settings(&SettingsPatch { interval: Some(name) }).await.map(drop) }),
				}
			}
			div { class: "mt-4 max-w-xl",
				Button {
					variant: ButtonVariant::Outline,
					size: Size::Sm,
					r#type: "button",
					onclick: move |_| act(|c| async move { c.reset_settings().await.map(drop) }),
					"Reset all to defaults"
				}
			}
		}
	}
}

/// One setting, offered `options` and whatever it holds besides.
#[component]
fn Choice(label: &'static str, hint: &'static str, options: Vec<u64>, setting: Setting<u64>, on_choose: EventHandler<String>) -> Element {
	let mut options = options;
	options.extend([setting.value(), setting.default]);
	options.sort_unstable();
	options.dedup();
	rsx! {
		div { class: "flex items-center gap-4 border-b border-border px-4 py-3 last:border-b-0",
			div { class: "flex min-w-0 flex-1 flex-col gap-0.5",
				span { "{label}" }
				span { class: "text-[12px] text-ink-soft", "{hint}" }
			}
			Select { value: Some(written(setting.value())), reset_value: Some(written(setting.default)), on_value_change: move |v| on_choose.call(v),
				SelectTrigger { size: Size::Sm, class: "w-24", SelectValue {} }
				SelectContent {
					for name in options.into_iter().map(written) {
						SelectItem { key: "{name}", value: name.clone(), text_value: name.clone(), "{name}" }
					}
				}
			}
		}
	}
}

/// `secs` in its largest whole unit, as the API reads it back.
fn written(secs: u64) -> String {
	match [(7 * 86_400, "w"), (86_400, "d"), (3600, "h"), (60, "m")]
		.into_iter()
		.find(|(unit, _)| secs.is_multiple_of(*unit))
	{
		Some((unit, suffix)) => format!("{}{suffix}", secs / unit),
		None => format!("{secs}s"),
	}
}
