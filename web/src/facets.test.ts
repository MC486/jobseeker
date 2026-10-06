import assert from "node:assert/strict";
import test from "node:test";
import type { JobListRow } from "./api.ts";
import { facetRail, matchesFacets, parseJobsSearch, shownStatus } from "./facets.ts";

function row(partial: Partial<JobListRow> & Pick<JobListRow, "id" | "status" | "work_mode">): JobListRow {
  return {
    title: partial.id,
    company_name: "Co",
    company_slug: "co",
    seniority: "unknown",
    salary_min_cents: null,
    salary_max_cents: null,
    salary_currency: null,
    salary_period: "unknown",
    posted_at: null,
    primary_location: null,
    extraction_partial: false,
    match_overall: null,
    skills_coverage: null,
    years_fit: null,
    user_rating: null,
    application_status: null,
    next_action: null,
    next_action_due: null,
    possibly_ghosted: false,
    closing_soon_unapplied: false,
    updated_at: "2026-10-06T00:00:00Z",
    ...partial,
  };
}

const rows = [
  row({ id: "open-remote", status: "open", work_mode: "remote", match_overall: 0.8 }),
  row({ id: "open-onsite", status: "open", work_mode: "onsite", match_overall: 0.4 }),
  row({
    id: "tracked",
    status: "open",
    work_mode: "unknown",
    application_status: "interested",
    match_overall: 0.2,
  }),
  row({ id: "unscored", status: "closed", work_mode: "remote", match_overall: null }),
];

test("pipeline status is what the table shows", () => {
  assert.equal(shownStatus(rows[2]), "interested");
  assert.equal(shownStatus(rows[0]), "open");
});

test("facets combine and a match floor drops unscored rows", () => {
  assert.equal(
    matchesFacets(rows[0], { status: "open", workMode: "remote", min: 70 }),
    true,
  );
  assert.equal(matchesFacets(rows[1], { min: 50 }), false);
  assert.equal(matchesFacets(rows[3], { workMode: "remote", min: 50 }), false);
  assert.equal(matchesFacets(rows[3], { workMode: "remote" }), true);
  assert.equal(matchesFacets(rows[2], { status: "interested" }), true);
  assert.equal(matchesFacets(rows[2], { status: "open" }), false);
});

test("each count ignores its own selection", () => {
  const rail = facetRail(rows, { status: "open", workMode: "remote", min: 50 });
  assert.deepEqual(
    rail.status.map((option) => [option.value, option.count]),
    [["open", 1]],
  );
  assert.deepEqual(
    rail.workMode.map((option) => [option.value, option.count]),
    [["remote", 1]],
  );
  assert.deepEqual(
    rail.match.map((option) => [option.value, option.count]),
    [
      [50, 1],
      [70, 1],
      [90, 0],
    ],
  );
});

test("search keeps only known facet values", () => {
  assert.deepEqual(
    parseJobsSearch({ q: "  rust ", sort: "skills", status: "interested", work_mode: "remote", min: "70" }),
    { q: "  rust ", sort: "skills", status: "interested", work_mode: "remote", min: 70 },
  );
  assert.deepEqual(parseJobsSearch({ status: "Applied", work_mode: "office", min: 40, sort: "nope" }), {});
});
