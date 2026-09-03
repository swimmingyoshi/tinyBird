/** Whether Contact has stopped accepting customer replies on this ticket. */
export function ticketIsLocked(status) {
  const state = String(status ?? "").trim().toLowerCase();
  return state === "resolved" || state === "closed";
}
