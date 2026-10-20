import { expect, it } from "vitest";
import { excerpt } from "../src/control/appdb.ts";
import { gitVersionAtLeast, isMissingObject, OFFLINE_GIT_MIN } from "../src/source/git.ts";

it("reads offline only on git 2.45 or newer, the first release that honours GIT_NO_LAZY_FETCH", () => {
  const cases = ["git version 2.39.5", "git version 2.44.2", "git version 2.45.0", "git version 2.54.0 (Apple Git-160)", "git version 3.0.1", "not git"];
  expect(cases.map((v) => gitVersionAtLeast(v, OFFLINE_GIT_MIN))).toEqual([false, false, true, true, true, false]);
});

it("marks a single line cut at the byte cap and never splits a code point", () => {
  const line = "é".repeat(5000);
  const text = excerpt(`${line}\n`, 1, 1)!;
  expect(text.endsWith(" … line truncated")).toBe(true);
  const kept = text.slice(0, -" … line truncated".length);
  expect(kept).toBe("é".repeat(4096));
  expect(kept.includes("�")).toBe(false);
});

it("tells a blob missing from the clone apart from other git failures", () => {
  const failures = [
    { stderr: "fatal: bad object 1234abcd" },
    { stderr: "error: unable to read 5678ef" },
    { stderr: "fatal: not a git repository: /data/repos/acme/shop.git" },
    { stderr: "" },
  ];
  expect(failures.map(isMissingObject)).toEqual([true, true, false, false]);
});
