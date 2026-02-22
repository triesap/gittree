#![forbid(unsafe_code)]

pub fn app_logging_init() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        tracing_wasm::set_as_global_default();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_logging_init_is_safe_to_call() {
        super::app_logging_init();
    }
}
