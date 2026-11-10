// The app's display name lives in exactly one place: `productName` in
// src-tauri/tauri.conf.json. The frontend reads it at build time.
import tauriConf from "../../src-tauri/tauri.conf.json";

export const APP_NAME: string = tauriConf.productName;
