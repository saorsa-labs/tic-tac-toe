import assert from "node:assert/strict";
import test from "node:test";

import {
  countBufferedTimelinePendingMessages,
  selectBufferedTimelineMessages,
} from "./useBufferedTimelineMessages.ts";

const rows = (...ids) => ids.map((id) => ({ id }));

test("freezes live arrivals after the semantic tail while scrolled up", () => {
  assert.deepEqual(
    selectBufferedTimelineMessages({
      frozenMessageIds: ["a", "b", "c"],
      isAtBottom: false,
      messages: rows("a", "b", "c", "d", "e"),
    }).map(({ id }) => id),
    ["a", "b", "c"],
  );
});

test("admits older-history prepends without exposing buffered arrivals", () => {
  assert.deepEqual(
    selectBufferedTimelineMessages({
      frozenMessageIds: ["a", "b", "c"],
      isAtBottom: false,
      messages: rows("older-a", "older-b", "a", "b", "c", "d"),
    }).map(({ id }) => id),
    ["older-a", "older-b", "a", "b", "c"],
  );
});

test("releases the full logical dataset at bottom", () => {
  const messages = rows("a", "b", "c", "d");
  assert.deepEqual(
    selectBufferedTimelineMessages({
      frozenMessageIds: ["a", "b"],
      isAtBottom: true,
      messages,
    }),
    messages,
  );
});

test("accepts an authoritative replacement when its old tail disappeared", () => {
  const messages = rows("x", "y");
  assert.deepEqual(
    selectBufferedTimelineMessages({
      frozenMessageIds: ["old-tail"],
      isAtBottom: false,
      messages,
    }),
    messages,
  );
});

test("countBufferedTimelinePendingMessages counts distinct new arrivals outside the frozen snapshot", () => {
  const cases = [
    {
      name: "dedupes a repeated frozen id and counts the single new arrival",
      frozenMessageIds: ["a", "a", "b"],
      isAtBottom: false,
      messages: rows("a", "b", "c"),
      expected: 1,
    },
    {
      name: "counts a duplicated current arrival only once",
      frozenMessageIds: ["a", "b"],
      isAtBottom: false,
      messages: rows("a", "b", "c", "c"),
      expected: 1,
    },
    {
      name: "returns zero when the reader is at the bottom",
      frozenMessageIds: ["a", "b"],
      isAtBottom: true,
      messages: rows("a", "b", "c", "d"),
      expected: 0,
    },
    {
      name: "returns zero when no snapshot is frozen",
      frozenMessageIds: null,
      isAtBottom: false,
      messages: rows("a", "b", "c"),
      expected: 0,
    },
    {
      name: "counts no pending when only older-history prepends precede the frozen tail",
      frozenMessageIds: ["a", "b", "c"],
      isAtBottom: false,
      messages: rows("older-a", "older-b", "a", "b", "c"),
      expected: 0,
    },
    {
      name: "counts only distinct live tail arrivals when a prepend and a duplicated tail both arrive",
      frozenMessageIds: ["a", "b", "c"],
      isAtBottom: false,
      messages: rows("older-a", "a", "b", "c", "d", "d", "e"),
      expected: 2,
    },
  ];
  for (const {
    name,
    frozenMessageIds,
    isAtBottom,
    messages,
    expected,
  } of cases) {
    assert.equal(
      countBufferedTimelinePendingMessages({
        frozenMessageIds,
        isAtBottom,
        messages,
      }),
      expected,
      name,
    );
  }
});
