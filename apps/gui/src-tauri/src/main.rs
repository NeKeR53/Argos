// Évite une fenêtre console supplémentaire sous Windows en release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    argos_gui_lib::run()
}
