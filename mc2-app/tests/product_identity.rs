//! The main-menu Exit button's HelpDesc is resource 27615.
#[test]
fn exit_help_uses_product_name() {
    let assets = mc2_ui::UiAssets::default();
    assert_eq!(
        assets.string(27615),
        format!("Quit {}.", mc2_platform::PRODUCT_NAME)
    );
}
