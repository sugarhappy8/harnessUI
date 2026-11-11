// Freezes Object.prototype in the app's own pages, before any other module runs.
//
// With it frozen, a mistake or a hostile value cannot add a property that every object
// then seems to have (prototype pollution). Each page of the app imports this file first.
//
// Tauri can do the same from its configuration (`freezePrototype`), but on Windows that
// runs in every frame, also in the preview frames of artifacts, where the diagram library
// cannot start with a frozen prototype. So the setting is off and the freeze is here: in
// the app's pages only. A test in src-tauri/src/commands.rs keeps the two together.
Object.freeze(Object.prototype);

export {};
