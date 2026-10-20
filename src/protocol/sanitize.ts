// Terminal-safe text: escape sequences (CSI, OSC, DCS/SOS/PM/APC strings, two-byte ESC) are removed whole, in both their
// 7-bit (ESC-prefixed) and 8-bit C1 forms, then every remaining C0/C1 control and DEL goes, except \n and \t.
// Both sides apply exactly this; protocol/README.md restates it for the Rust client.
const SEQUENCES =
  /(?:\u001b\[|\u009b)[0-?]*[ -/]*[@-~]?|(?:\u001b[\]PX^_]|[\u009d\u0090\u0098\u009e\u009f])[^\u0007\u001b\u009c]*(?:\u0007|\u001b\\|\u009c)?|\u001b[ -/]*[0-~]?/g;
const CONTROLS = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g;

export function sanitize(text: string): string {
  return text.replace(SEQUENCES, "").replace(CONTROLS, "");
}
