// A message shown under a button, or in a toast: the outcome of the last action.
export interface ActionMessage {
  id: string;             // the button it belongs to, e.g. "deposit", "borrow"
  text: string;
  kind: "busy" | "good" | "error";
}
