import assert from "node:assert/strict";
import { applyFilters, authorsOf, authorKey, reposOf } from "../src/reviewFilter.ts";

const pr = (n: number, author: any, bucket: string, repo = "r") => ({ key: `o/${repo}#${n}`, owner: "o", repo, author, bucket }) as any;
const amy = { kind: "linear", id: "u1", name: "Amy", avatarUrl: null };
const bo = { kind: "linear", id: "u2", name: "Bo", avatarUrl: null };
const anon = { kind: "unlinked", login: "zed" };
const prs = [pr(1, amy, "draft"), pr(2, bo, "needsReview"), pr(3, anon, "draft"), pr(4, amy, "approved")];
const N = { authors: [], statuses: [], repos: [] } as any;
const keys = (v: any[]) => v.map((p) => p.key);

assert.deepEqual(keys(applyFilters(prs, { ...N })), ["o/r#1", "o/r#2", "o/r#3", "o/r#4"]); // empty = no filter
assert.deepEqual(keys(applyFilters(prs, { ...N, authors: ["u1", "unlinked"] })), ["o/r#1", "o/r#3", "o/r#4"]); // OR within
assert.deepEqual(keys(applyFilters(prs, { ...N, authors: ["u1"], statuses: ["draft"] })), ["o/r#1"]); // AND across
assert.deepEqual(keys(applyFilters(prs, { ...N, statuses: ["draft", "approved"] })), ["o/r#1", "o/r#3", "o/r#4"]);
assert.equal(authorKey(anon as any), "unlinked");
assert.deepEqual(authorsOf(prs).map((a) => a.key), ["u1", "u2", "unlinked"]); // by name, unlinked last, deduped
const two = [...prs, pr(5, bo, "draft", "web")];
assert.deepEqual(keys(applyFilters(two, { ...N, repos: ["o/web"] })), ["o/web#5"]); // repo filter
assert.deepEqual(keys(applyFilters(two, { ...N, repos: ["o/web", "o/r"], statuses: ["draft"] })), ["o/r#1", "o/r#3", "o/web#5"]); // OR repos, AND status
assert.deepEqual(reposOf(two), ["o/r", "o/web"]);
assert.equal(applyFilters(two, { authors: [], statuses: [] } as any).length, 5, "old saved filters without repos still work");
console.log("reviewFilter ok");
