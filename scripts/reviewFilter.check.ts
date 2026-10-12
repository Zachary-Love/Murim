import assert from "node:assert/strict";
import { applyFilters, authorsOf, authorKey } from "../src/reviewFilter.ts";

const pr = (n: number, author: any, bucket: string) => ({ key: `o/r#${n}`, author, bucket }) as any;
const amy = { kind: "linear", id: "u1", name: "Amy", avatarUrl: null };
const bo = { kind: "linear", id: "u2", name: "Bo", avatarUrl: null };
const anon = { kind: "unlinked", login: "zed" };
const prs = [pr(1, amy, "draft"), pr(2, bo, "needsReview"), pr(3, anon, "draft"), pr(4, amy, "approved")];
const keys = (v: any[]) => v.map((p) => p.key);

assert.deepEqual(keys(applyFilters(prs, { authors: [], statuses: [] })), ["o/r#1", "o/r#2", "o/r#3", "o/r#4"]); // empty = no filter
assert.deepEqual(keys(applyFilters(prs, { authors: ["u1", "unlinked"], statuses: [] })), ["o/r#1", "o/r#3", "o/r#4"]); // OR within
assert.deepEqual(keys(applyFilters(prs, { authors: ["u1"], statuses: ["draft"] })), ["o/r#1"]); // AND across
assert.deepEqual(keys(applyFilters(prs, { authors: [], statuses: ["draft", "approved"] })), ["o/r#1", "o/r#3", "o/r#4"]);
assert.equal(authorKey(anon as any), "unlinked");
assert.deepEqual(authorsOf(prs).map((a) => a.key), ["u1", "u2", "unlinked"]); // by name, unlinked last, deduped
console.log("reviewFilter ok");
