//! Gadget actions, their default shortcuts and what the settings window says about each gadget.
use super::layout::SnapAction::{self, *};
use super::settings::*;
use super::shortcut::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Action { pub id: String, pub module: ModuleId, pub title: String, pub default: Option<Shortcut> }

const CA: u32 = CTRL | ALT;
const fn vk(c: char) -> u32 { c as u32 }
const LEFT: u32 = 0x25; const UP: u32 = 0x26; const RIGHT: u32 = 0x27; const DOWN: u32 = 0x28;

pub fn snap_default(a: SnapAction) -> Option<Shortcut> {
    let s = |k, m| Some(Shortcut::new(k, m));
    match a {
        Left => s(LEFT, CA), Right => s(RIGHT, CA), TopHalf => s(UP, CA), BottomHalf => s(DOWN, CA),
        TopLeft => s(vk('U'), CA), TopRight => s(vk('I'), CA), BottomLeft => s(vk('J'), CA), BottomRight => s(vk('K'), CA),
        FirstThird => s(vk('D'), CA), CenterThird => s(vk('F'), CA), LastThird => s(vk('G'), CA),
        FirstTwoThirds => s(vk('E'), CA), CenterTwoThirds => s(vk('R'), CA), LastTwoThirds => s(vk('T'), CA),
        Maximize => s(0x0D, CA), MaximizeHeight => s(UP, CA | SHIFT), Center => s(vk('C'), CA),
        Smaller => s(0xBD, CA), Larger => s(0xBB, CA), Restore => s(0x08, CA),
        NextDisplay => s(RIGHT, CA | WIN), PreviousDisplay => s(LEFT, CA | WIN),
        CenterHalf | AlmostMaximize => None,
    }
}

pub fn snap_id(a: SnapAction) -> String { format!("snap.{}", a.id()) }

pub fn actions() -> Vec<Action> {
    let mut v: Vec<Action> = SnapAction::ALL.iter().map(|a| Action { id: snap_id(*a), module: ModuleId::WindowSnap, title: a.name().into(), default: snap_default(*a) }).collect();
    let mut add = |id: &str, m, t: &str, d| v.push(Action { id: id.into(), module: m, title: t.into(), default: d });
    use ModuleId::*;
    add("tray.toggle", TrayOrganizer, "Show / Hide Tray Icons", None);
    add("shelf.toggle", FileShelf, "Show / Hide Shelf", Some(Shortcut::new(vk('S'), CA)));
    add("shelf.paste", FileShelf, "Add Clipboard to Shelf", None);
    add("capture.area", CaptureFloat, "Capture Area", Some(Shortcut::new(vk('2'), CA)));
    add("capture.previous", CaptureFloat, "Capture Previous Area", None);
    add("capture.window", CaptureFloat, "Capture Window", None);
    add("capture.screen", CaptureFloat, "Capture Screen", None);
    add("capture.text", CaptureFloat, "Copy Text from Area", None);
    add("capture.lock", CaptureFloat, "Lock / Unlock All Floats", None);
    add("capture.hide", CaptureFloat, "Hide / Show All Floats", None);
    add("capture.closeAll", CaptureFloat, "Close All Floats", None);
    add("capture.color", CaptureFloat, "Pick a Colour from the Screen", None);
    add("capture.live", CaptureFloat, "Float a Live Part of a Window", None);
    add("dim.toggle", FocusDim, "Pause / Resume Dimming", Some(Shortcut::new(vk('H'), CA | WIN)));
    add("awake.toggle", KeepAwake, "Keep Awake On / Off", None);
    add("clipboard.show", Clipboard, "Show Clipboard History", Some(Shortcut::new(vk('V'), CA)));
    add("clipboard.plain", Clipboard, "Paste Last Copy as Plain Text", None);
    add("device.micMute", DeviceControl, "Mute / Unmute Microphone", Some(Shortcut::new(vk('M'), CA)));
    add("device.nextOutput", DeviceControl, "Next Sound Output", None);
    add("device.brighter", DeviceControl, "Brighten External Displays", None);
    add("device.dimmer", DeviceControl, "Darken External Displays", None);
    add("diskmap.open", DiskMap, "Open Disk Map", None);
    add("appswitch.next", AppSwitch, "Switch Windows", Some(super::appswitch::NEXT_DEFAULT));
    add("appswitch.previous", AppSwitch, "Switch Windows Backward", Some(super::appswitch::PREVIOUS_DEFAULT));
    add("appswitch.hideOthers", AppSwitch, "Hide Other Windows", None);
    add("clock.show", CalendarClock, "Show Calendar", None);
    add("device.theme", DeviceControl, "Switch Light / Dark Mode", None);
    add("mouse.reverse", MouseTune, "Reverse Mouse Scrolling On / Off", None);
    add("window.onTop", WindowSnap, "Keep Window on Top", Some(Shortcut::new(vk('T'), CTRL | WIN)));
    for item in MonitorItem::ALL {
        add(&format!("monitor.{}", item.name().to_lowercase()), SystemMonitor, &format!("Open {} Panel", item.name()), None);
    }
    v
}

pub const SNAP_GROUPS: [(&str, &[SnapAction]); 6] = [
    ("Halves", &[Left, Right, CenterHalf, TopHalf, BottomHalf]),
    ("Quarters", &[TopLeft, TopRight, BottomLeft, BottomRight]),
    ("Thirds", &[FirstThird, CenterThird, LastThird]),
    ("Two Thirds", &[FirstTwoThirds, CenterTwoThirds, LastTwoThirds]),
    ("Size and Position", &[Maximize, AlmostMaximize, MaximizeHeight, Center, Smaller, Larger, Restore]),
    ("Displays", &[NextDisplay, PreviousDisplay]),
];

impl AppSettings {
    /// The user's choice, else the default.
    pub fn shortcut_for(&self, a: &Action) -> Option<Shortcut> {
        match self.action_shortcuts.get(&a.id) { Some(c) => *c, None => a.default }
    }
    /// Everything in use except `except`, named for a clash message.
    pub fn taken_shortcuts(&self, except: &str) -> Vec<(String, Shortcut)> {
        let mut v: Vec<(String, Shortcut)> = actions().iter().filter(|a| a.id != except)
            .filter_map(|a| self.shortcut_for(a).map(|s| (format!("{} ▸ {}", a.module.name(), a.title), s))).collect();
        for (m, s) in &self.tool_shortcuts {
            if format!("open.{}", m.key()) != except { v.push((format!("{} ▸ Open Controls", m.name()), *s)); }
        }
        for h in &self.options.app_shortcuts {
            if let Some(s) = h.shortcut { if super::appswitch::app_id(&h.path) != except { v.push((format!("{} ▸ {} (App Switch)", ModuleId::AppSwitch.name(), h.name), s)); } }
        }
        v
    }
}

pub struct Feature { pub title: &'static str, pub detail: &'static str }
pub struct Tip { pub keys: &'static str, pub gesture: &'static str, pub title: &'static str }
pub struct Info {
    pub tagline: &'static str,
    pub features: &'static [Feature],
    pub tips: &'static [Tip],
    pub note: &'static str,
    pub limits: &'static [&'static str],
}

macro_rules! f { ($t:expr, $d:expr) => { Feature { title: $t, detail: $d } } }
macro_rules! t { ($k:expr, $g:expr, $d:expr) => { Tip { keys: $k, gesture: $g, title: $d } } }

pub fn info(id: ModuleId) -> &'static Info {
    match id {
        ModuleId::TrayOrganizer => &Info {
            tagline: "Keeps the taskbar corner tidy: tray icons you rarely need fold away, and come back when you ask.",
            features: &[
                f!("Fold Away Icons", "One click sends every icon you did not keep into Windows' own overflow menu; another brings them all back."),
                f!("You Choose What Stays", "Tick the icons that always stay in view. The list shows every app Windows has seen in the tray."),
                f!("Auto-Collapse", "Folds the icons away again after 5 to 60 seconds."),
                f!("Shortcut", "An optional key to show or hide them."),
            ],
            tips: &[t!("", "Click the panel button", "Show or hide tray icons")],
            note: "No administrator rights needed. It changes the same per-user switch as Settings ▸ Personalization ▸ Taskbar ▸ Other system tray icons, and puts every icon back as it was when you switch it off.",
            limits: &[
                "Windows has no public interface for this: the switch is an undocumented per-user setting, so a future Windows update could stop it. The gadget then says so and changes nothing.",
                "Windows 11 22H2 or later. Windows decides the order of the icons.",
            ],
        },
        ModuleId::SystemMonitor => &Info {
            tagline: "Live CPU, memory, disk, network, sensor and battery readings in the tray, with detail panels.",
            features: &[
                f!("Tray Icons", "Pick any of CPU, memory, disk, network, sensors and battery. Each is a small live number, updated every 2 seconds."),
                f!("Detail Panels", "Click an icon for history charts, per-core load, top processes, volumes, interfaces and temperatures."),
                f!("Chart Ranges", "10 minutes to 24 hours of history, kept in memory only."),
                f!("Battery", "Charge, time remaining, cycle count, health and power."),
                f!("Alerts", "Sustained high CPU, memory pressure, a nearly full disk, a hot sensor and a low battery. Notifications are optional."),
                f!("Public IP on Request", "Looked up only when you press Look Up or turn on automatic lookup."),
            ],
            tips: &[t!("", "Click an icon", "Open its panel"), t!("Esc", "", "Close the panel")],
            note: "No administrator rights needed. Readings come from Windows performance counters. Process lists show what your account may read.",
            limits: &[
                "Windows has no public interface for fan speeds or per-core temperatures without a kernel driver, which this app refuses to install. Only what the firmware reports is shown; the rest reads “–”.",
                "Per-process network traffic needs administrator rights, so it is not shown. Per-process disk activity is.",
                "Windows has no load average: the panel shows the processor queue length instead.",
            ],
        },
        ModuleId::WindowSnap => &Info {
            tagline: "Snap windows to halves, quarters, thirds and more, from the keyboard or by dragging to a screen edge.",
            features: &[
                f!("24 Window Commands", "Halves, quarters, thirds, two thirds, center, maximize, almost maximize, maximize height, larger and smaller."),
                f!("Repeat to Cycle", "Press a half again for two thirds, then one third; or move on to the next display."),
                f!("Drag to Snap", "Corners give quarters, the top maximizes, the bottom gives thirds. A preview shows where it lands."),
                f!("Restore", "Ctrl+Alt+Backspace, or dragging a snapped window away, brings back the size it had before."),
                f!("Displays", "Ctrl+Alt+Win+← and → move a window to the neighbouring display and keep its proportions."),
                f!("Window Motion", "Ctrl+Alt-drag anywhere inside a window to move it, Ctrl+Alt-right-drag to resize it."),
                f!("Gaps", "Leave 4 to 24 pixels between snapped windows and the screen edges."),
                f!("Keep on Top", "Win+Ctrl+T keeps the front window above all others, with a yellow frame; press it again to release."),
                f!("Layouts Come Back", "Unplug a monitor and plug it in again: windows go back to where they were on that set of displays."),
            ],
            tips: &[t!("Ctrl+Alt", "Drag", "Move the window under the pointer"), t!("Ctrl+Alt", "Right-Drag", "Resize it from the nearest corner"), t!("", "Drag to an edge", "Snap there")],
            note: "No administrator rights needed.",
            limits: &[
                "Windows' own Snap also reacts to edge drags. While “Snap windows” is on in Settings ▸ System ▸ Multitasking, edge drags are left to Windows; the keyboard commands always work.",
                "Windows protects windows running as administrator: they cannot be moved unless this app also runs as administrator.",
                "Some apps keep a minimum size, so their windows may not shrink all the way.",
                "On keyboard layouts with AltGr, Ctrl+Alt+letter also types characters: change those shortcuts if they get in the way.",
            ],
        },
        ModuleId::FileShelf => &Info {
            tagline: "A floating shelf to park files, images, links and text while you move between apps.",
            features: &[
                f!("Shake to Open", "Shake the pointer while dragging and the shelf appears. It can also open on any drag, or only from its shortcut."),
                f!("Holds Anything", "Files and folders, attachments and images from browsers and Outlook, web links and text."),
                f!("Drag Out Again", "Drag one item, or all of them at once, into any app. Dragging out always copies."),
                f!("Preview", "Select an item and press Space to preview it."),
                f!("Space in File Explorer", "Select a file in File Explorer and press Space: a preview opens, as Quick Look does on a Mac. Space again closes it."),
                f!("Undo", "Ctrl+Z brings back items you removed or cleared."),
                f!("Clipboard", "Ctrl+V on the shelf adds the clipboard; an optional shortcut does it from anywhere."),
            ],
            tips: &[t!("Space", "", "Preview the selected item"), t!("Ctrl+V", "", "Add the clipboard"), t!("Ctrl+C", "", "Copy the selected item"), t!("Ctrl+Z", "", "Bring back what you removed"), t!("Del", "", "Remove the selected item"), t!("", "Double-Click", "Open it")],
            note: "No administrator rights needed. Nothing is read unless you drop it on the shelf.",
            limits: &[
                "Space previews work in File Explorer windows, not on the desktop.",
                "Windows has no public signal for “a drag is in progress”. The shelf recognises the drag image Windows draws, so apps that drag without one need the shortcut.",
                "Items stay until you remove them or switch the gadget off. Images and text are kept as copies in the app's own folder and deleted then.",
            ],
        },
        ModuleId::CaptureFloat => &Info {
            tagline: "Capture part of the screen and keep it floating above your work as a reference.",
            features: &[
                f!("Capture", "An area, a window, the whole screen, or the previous area again. Press Space while selecting to pick a window."),
                f!("Floats on Top", "Stays above other windows. Move it, Ctrl-scroll or drag the corner to resize."),
                f!("Opacity", "Scroll over a float to fade it, to compare it with what is underneath."),
                f!("Lock", "Make floats click-through so you can work under them; unlock them from the panel or a shortcut."),
                f!("Copy Text", "Recognises text and QR codes in a float, on this PC, and copies them."),
                f!("Drag, Copy, Save", "Drag the handle into any app, Ctrl+C to copy, Ctrl+S to save as PNG."),
                f!("Close or Hide All", "Esc closes one float; the panel or a shortcut closes or hides them all."),
                f!("Draw on It", "Boxes, arrows, a pen, text and mosaic in six colours (Ctrl+E). What you draw goes along when you copy, save or drag it."),
                f!("Pick a Colour", "A loupe over the screen; click to copy the colour as #RRGGBB."),
                f!("Live Float", "Keep part of another window floating and moving: a video, a chart, a build log. Windows draws it, so it costs almost nothing."),
            ],
            tips: &[t!("Space", "While selecting", "Pick a window instead of an area"), t!("Ctrl+C", "", "Copy the float"), t!("Ctrl+S", "", "Save it as PNG"), t!("Esc", "", "Close it"), t!("", "Scroll", "Change its opacity"), t!("Ctrl", "Scroll", "Resize it"), t!("", "Double-Click", "Close it"), t!("Ctrl+E", "", "Draw on it"), t!("Ctrl+Z", "", "Undo a mark")],
            note: "No administrator rights needed. Nothing is recorded or uploaded; captures stay in memory until you save or copy them.",
            limits: &[
                "Print Screen and Win+Shift+S stay with Windows.",
                "Apps that protect their content (some video players and banking apps) capture as black.",
                "Copy Text uses the OCR languages installed in Windows. Add one in Settings ▸ Time & language ▸ Language.",
            ],
        },
        ModuleId::FocusDim => &Info {
            tagline: "Dims everything behind the window you are working in, so it stands out.",
            features: &[
                f!("Spotlight the Active Window", "Other windows and the desktop darken; the taskbar stays clear."),
                f!("Strength", "From a soft shade at 10% to a dark backdrop at 90%, live."),
                f!("Pause", "Ctrl+Alt+Win+H pauses and resumes dimming without switching the gadget off."),
                f!("Whole App", "Keep every window of the active app bright instead of only the front one."),
                f!("Displays", "Dim the other displays too, or only the one you are working on."),
                f!("Fade", "Fades between windows; instant while animations are off in Windows."),
                f!("Clicks Pass Through", "The shade never catches a click. Clicking the desktop shows everything."),
            ],
            tips: &[t!("", "Click the desktop", "Show every window undimmed")],
            note: "No administrator rights needed. Window positions come from the system; titles and contents are never read.",
            limits: &["Always-on-top windows of other apps stay bright."],
        },
        ModuleId::KeepAwake => &Info {
            tagline: "Keeps the PC awake for a while, while an app runs, or until you turn it off.",
            features: &[
                f!("One Click", "Click the cup in the taskbar corner to keep the PC awake; click again to let it sleep."),
                f!("For a While", "15 minutes to 5 hours, or until you turn it off. The cup's tooltip shows the time left."),
                f!("While an App Runs", "Stays awake while chosen programs run, such as a download manager or a video call."),
                f!("Display Too", "Keep the display on, or let it turn off while the PC keeps working."),
                f!("Battery Guard", "Lets the PC sleep again when the battery runs low."),
            ],
            tips: &[t!("", "Click the cup", "Keep awake on or off"), t!("", "Right-click the cup", "Choose for how long")],
            note: "No administrator rights needed. It asks Windows to stay awake the way video players do, and stops asking when it stops.",
            limits: &["Closing the lid or pressing the power button still sleeps the PC: Windows decides those in Power Options.", "Company policies can force sleep regardless."],
        },
        ModuleId::Clipboard => &Info {
            tagline: "Remembers what you copy so you can find it and paste it again.",
            features: &[
                f!("History", "Text, links, images and files you copy, newest first, up to the number you choose."),
                f!("Search and Paste", "Ctrl+Alt+V opens the list at the pointer; type to search, Enter pastes into the app you were in."),
                f!("Pinned", "Pin what you paste often (an address, a signature); pins stay at the top and are kept after a restart."),
                f!("Plain Text", "Shift+Enter pastes without formatting; a shortcut pastes your last copy as plain text."),
                f!("To the Shelf", "Send an item to File Shelf to drag it somewhere later."),
                f!("Passwords Stay Out", "Copies that password managers mark as private are never recorded."),
            ],
            tips: &[t!("Ctrl+Alt+V", "", "Open the history"), t!("Enter", "", "Paste the selected item"), t!("Shift+Enter", "", "Paste as plain text"), t!("Ctrl+1", "", "Paste one of the first nine"), t!("Ctrl+P", "", "Pin or unpin"), t!("Del", "", "Remove it"), t!("Ctrl+S", "", "Send it to File Shelf")],
            note: "No administrator rights needed. The history lives in memory; only pins are written to disk, in the app's own folder.",
            limits: &["Windows' own history (Win+V) keeps working beside it.", "Apps running as administrator do not accept a paste from it; the item is still copied, so press Ctrl+V there."],
        },
        ModuleId::DiskMap => &Info {
            tagline: "See what fills a disk, go to it, and move what you no longer need to the Recycle Bin.",
            features: &[
                f!("What Is Left", "The free space in large type, and the whole disk as blocks sized by what they hold."),
                f!("Blocks", "Every folder is a block with its biggest folders and files inside; double-click goes in, Backspace comes back."),
                f!("Go to It", "Open an item, show it in File Explorer or copy its path from the card beside the map."),
                f!("Recycle With Your Approval", "Move to Recycle Bin asks first, names what goes and how big it is, and checks it again right before."),
                f!("Never Windows Itself", "Windows, Program Files, your profile folders and other accounts are never offered."),
            ],
            tips: &[t!("", "Double-Click", "Go inside"), t!("Backspace", "", "Go up"), t!("Del", "", "Move to Recycle Bin…"), t!("F5", "", "Scan again"), t!("Ctrl+C", "", "Copy the path")],
            note: "No administrator rights needed. Folders your account may not read are counted as not readable and left out.",
            limits: &[
                "Files that live only in the cloud (OneDrive placeholders) take no space on this disk and are left out.",
                "Moving to the Recycle Bin frees the space only once the Recycle Bin is emptied.",
                "A full scan of a large drive takes about a minute; the map stays for ten minutes after you close it.",
            ],
        },
        ModuleId::AppSwitch => &Info {
            tagline: "Goes through every window, not only apps, with a shortcut you hold and let go of.",
            features: &[
                f!("Window Switcher", "Hold Ctrl+Alt and press ` to go through every window; let go to switch to the one you stopped on."),
                f!("Backward Too", "Add Shift to go the other way round the list."),
                f!("Minimized Windows Too", "Minimized windows are listed after the open ones and come back when you pick them."),
                f!("Pick with the Pointer", "Click a window in the switcher to go straight to it."),
                f!("Hide Other Windows", "One shortcut minimizes every window except the front app's."),
            ],
            tips: &[t!("Ctrl+Alt+`", "", "Next window"), t!("Ctrl+Alt+Shift+`", "", "Previous window"), t!("", "Let go of Ctrl+Alt", "Switch to it"), t!("Esc", "", "Cancel")],
            note: "No administrator rights needed. Windows' own Alt+Tab keeps working; this has shortcuts of its own, which you can change on the Shortcuts tab.",
            limits: &[
                "Windows keeps Alt+Tab for itself, so the default is Ctrl+Alt+` instead of the Mac's Option+Tab.",
                "Windows of apps running as administrator can be listed but may refuse to come to the front.",
                "Windows on other virtual desktops are not listed.",
            ],
        },
        ModuleId::CalendarClock => &Info {
            tagline: "A clock with a calendar and world clocks in the taskbar corner.",
            features: &[
                f!("Your Clock", "Date, weekday and time the way you like them, with seconds if you want, in the icon's tooltip."),
                f!("Month at a Glance", "Click the icon for a month calendar; move between months and jump back to today."),
                f!("World Clocks", "The time in the cities you add, with how far ahead or behind they are."),
            ],
            tips: &[t!("", "Click the icon", "Open the calendar"), t!("", "Hover the icon", "Read the clock")],
            note: "No administrator rights needed and nothing to allow. It only reads the PC's clock; nothing is sent anywhere.",
            limits: &[
                "Windows has no place for text in the taskbar corner, so the clock is the icon's tooltip and the icon shows today's date. The taskbar's own clock stays; hide its date in Settings ▸ Personalization ▸ Taskbar if you do not want two.",
                "Your calendar events, the countdown to the next meeting and the Join button are not here: Windows has no calendar every app shares.",
                "World clocks know the daylight-saving rules of the listed cities as they are today; a city that changes its rules needs an update of this app.",
            ],
        },
        ModuleId::MouseTune => &Info {
            tagline: "Sets up a wheel mouse your way: its own scroll direction, scroll speed, pointer speed and pointer precision.",
            features: &[
                f!("Own Scroll Direction", "Reverse the wheel of a mouse while a precision touchpad keeps the direction Windows gives it."),
                f!("Scroll Speed", "Half to three times as many lines per notch as your Windows setting."),
                f!("Pointer Speed", "Pick a pointer speed from here, or leave it as Windows has it."),
                f!("Pointer Precision", "Turn Enhance pointer precision on or off, or leave it alone."),
                f!("Your Values Come Back", "Switch the gadget off, quit the app, or restart after a crash: the values you had before return, unless you changed them yourself meanwhile."),
            ],
            tips: &[t!("", "Tray panel", "Reverse the wheel with one click")],
            note: "No administrator rights needed. Pointer speed, scroll lines and precision are the per-user settings of Settings ▸ Bluetooth & devices ▸ Mouse; they last until you sign out even if the app is killed, and the app puts them back as it stops.",
            limits: &[
                "Smooth scrolling, side-button actions and a middle-button action are not offered: Windows has no system setting for smooth wheel scrolling (apps do it themselves), side buttons already go back and forward in browsers and File Explorer, and there is no Mission Control to open.",
                "Only wheel notches are reversed. A precision touchpad or a free-spinning wheel sends finer steps and keeps its own direction; change those in Windows' touchpad settings.",
                "Windows apps running as administrator do not receive reversed wheel events from an app that is not.",
                "Not checked on a physical mouse or touchpad yet.",
            ],
        },
        ModuleId::DeviceControl => &Info {
            tagline: "Switches sound devices, mutes the microphone and sets external display brightness.",
            features: &[
                f!("Sound Output", "Pick speakers or headphones and set their volume from one place."),
                f!("Microphone Mute", "Ctrl+Alt+M mutes the microphone everywhere; a red microphone in the taskbar corner shows it is muted."),
                f!("Preferred Devices", "Switch to your headset or microphone by themselves when they connect."),
                f!("Next Output", "One shortcut moves the sound on to the next device, round the list."),
                f!("External Displays", "Brightness of external monitors over DDC/CI, a slider each and shortcuts for all."),
                f!("Volume per App", "A slider and a mute button for every program that plays sound, like the Volume Mixer but one click away."),
                f!("Light and Dark on a Schedule", "Windows switches to light mode in the morning and dark mode in the evening, at the times you choose."),
            ],
            tips: &[t!("Ctrl+Alt+M", "", "Mute or unmute the microphone"), t!("", "Click the red microphone", "Unmute it")],
            note: "No administrator rights needed. Devices are switched through Windows' own audio settings; nothing is recorded.",
            limits: &[
                "Choosing the default device uses the same interface Windows' Sound settings use, which Microsoft does not document. A future Windows could change it; the gadget then says so.",
                "Brightness works on monitors that accept DDC/CI (turn it on in the monitor's own menu). A laptop's built-in screen stays with Windows' brightness slider.",
            ],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn every_snap_action_is_in_one_group() {
        let all: Vec<_> = SNAP_GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
        assert_eq!(all.len(), 24);
        for a in SnapAction::ALL { assert_eq!(all.iter().filter(|x| **x == a).count(), 1); }
    }
    #[test] fn every_gadget_has_a_page_text() {
        for m in ModuleId::ALL { let i = info(m); assert!(!i.tagline.is_empty() && !i.features.is_empty() && !i.note.is_empty(), "{}", m.name()); }
    }
    #[test] fn calendar_clock_is_listed_with_a_show_action_and_no_default_key() {
        let a = actions().into_iter().find(|a| a.id == "clock.show").unwrap();
        assert_eq!((a.module, a.default), (ModuleId::CalendarClock, None));
    }
    #[test] fn defaults_do_not_clash_and_pass_the_policy() {
        let s = AppSettings::default();
        for a in actions() {
            if let Some(d) = a.default {
                assert_eq!(validate(d, &s.taken_shortcuts(&a.id)), None, "{}", a.id);
            }
        }
    }
    #[test] fn user_choice_wins_and_none_removes() {
        let mut s = AppSettings::default();
        let a = actions().into_iter().find(|a| a.id == "snap.left").unwrap();
        s.action_shortcuts.insert(a.id.clone(), None);
        assert_eq!(s.shortcut_for(&a), None);
        assert!(!s.taken_shortcuts("x").iter().any(|t| t.1 == a.default.unwrap()));
    }
}
