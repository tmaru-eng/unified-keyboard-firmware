//! Executable-demo acceptance test.

#[path = "../examples/bridge_demo.rs"]
mod bridge_demo;

#[test]
fn demo_prints_the_expected_bridge_lifecycle() {
    let mut output = Vec::new();

    bridge_demo::write_demo(&mut output).expect("the in-memory demo output should be writable");

    let output = String::from_utf8(output).expect("the demo output should be UTF-8");
    assert_eq!(
        output,
        concat!(
            "attached: usb=source-0 ble=source-1\n",
            "ble-us-jis-at: [00, 00, 2f, 00, 00, 00, 00, 00]\n",
            "usb-a: [00, 00, 04, 00, 00, 00, 00, 00]\n",
            "usb-a+ble-b: [00, 00, 04, 05, 00, 00, 00, 00]\n",
            "detach-usb-keeps-ble-b: [00, 00, 05, 00, 00, 00, 00, 00]\n",
        )
    );
}
