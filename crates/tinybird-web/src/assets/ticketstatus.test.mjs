import assert from "node:assert/strict";
import test from "node:test";

import { ticketIsLocked } from "./ticketstatus.js";

test("open ticket states accept customer replies", () => {
  for (const state of [undefined, "", "new", "open", "pending"]) {
    assert.equal(ticketIsLocked(state), false, String(state));
  }
});

test("resolved and closed tickets do not offer a reply", () => {
  for (const state of ["resolved", "Resolved", " CLOSED "]) {
    assert.equal(ticketIsLocked(state), true, state);
  }
});
