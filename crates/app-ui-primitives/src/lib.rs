#![forbid(unsafe_code)]

pub use ui_primitives_core::dialog::DialogModel;
pub use ui_primitives_core::roving_focus::{
    roving_focus_action_from_key as gittree_app_ui_roving_focus_action_from_key,
    roving_focus_next_index as gittree_app_ui_roving_focus_next_index,
    RovingFocusAction as GittreeAppUiRovingFocusAction,
    RovingFocusOrientation as GittreeAppUiRovingFocusOrientation,
};
pub use ui_primitives_leptos::builders::{
    dialog_content_attrs,
    dialog_trigger_attrs,
};
pub use ui_primitives_leptos::{
    dismissable_is_escape as gittree_app_ui_dismissable_is_escape,
    dismissable_is_outside as gittree_app_ui_dismissable_is_outside,
    focus_scope_next_index as gittree_app_ui_focus_scope_next_index,
    focus_scope_selector as gittree_app_ui_focus_scope_selector,
    modal_hide_siblings as gittree_app_ui_modal_hide_siblings,
    modal_restore as gittree_app_ui_modal_restore,
    presence_state_next as gittree_app_ui_presence_state_next,
    scroll_lock_acquire as gittree_app_ui_scroll_lock_acquire,
    scroll_lock_release as gittree_app_ui_scroll_lock_release,
    use_primitive,
    DismissableLayer as GittreeAppUiDismissableLayer,
    DismissableReason as GittreeAppUiDismissableReason,
    FocusScope as GittreeAppUiFocusScope,
    ModalError as GittreeAppUiModalError,
    ModalGuard as GittreeAppUiModalGuard,
    ModalResult as GittreeAppUiModalResult,
    ModalTarget as GittreeAppUiModalTarget,
    Portal as GittreeAppUiPortal,
    PortalMount as GittreeAppUiPortalMount,
    Presence as GittreeAppUiPresence,
    PresenceState as GittreeAppUiPresenceState,
    PrimitiveAttribute,
    PrimitiveAttributeValue,
    PrimitiveElement,
    PrimitiveError,
    PrimitiveEvent,
    PrimitiveResult,
    ScrollLockError as GittreeAppUiScrollLockError,
    ScrollLockGuard as GittreeAppUiScrollLockGuard,
    ScrollLockResult as GittreeAppUiScrollLockResult,
};
