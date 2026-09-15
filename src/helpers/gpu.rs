// SPDX-License-Identifier: GPL-3.0-or-later

pub fn get_dri_node() -> (String, String) {
    let mut render = String::new();

    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        let mut nodes: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_string_lossy().to_string())
            .filter(|p| p.contains("renderD"))
            .collect();
        nodes.sort();
        if let Some(node) = nodes.first() {
            render = node.clone();
        }
    }

    let driver = detect_driver(&render);

    (render, driver)
}

fn detect_driver(render: &str) -> String {
    if render.is_empty() {
        return String::new();
    }
    String::new()
}

pub fn get_vulkan_driver(render_node: &str) -> Option<String> {
    if render_node.is_empty() {
        return None;
    }
    None
}
