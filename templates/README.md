# Templates

Two spreadsheet-shaped things opcode reads or writes, and the example file for
each.

## Ticket import

`tickets-template.csv` is a starting point for the board's **Import** button
(Tasks → Import, then *Save an example sheet*). Open it in Excel, Numbers or a
text editor, replace the rows, and import the result — `.csv`, `.tsv`, `.xlsx`,
`.xlsm`, `.xls`, `.xlsb` and `.ods` all work.

## Columns

The first non-empty row is read as headings. Only `title` is required.

| Column | Meaning |
| --- | --- |
| `title` | What the ticket is called. A row without one is skipped. |
| `epic` | Grouping label, shown under the title on the card. |
| `description` | **Becomes the agent's task prompt**, falling back to the title when empty. The most useful column to fill in properly. |
| `priority` | `low`, `medium`, `high` or `critical`. Empty means `medium`. |

Headings are matched on their letters and digits alone, so `Epic`, `epic` and
`  EPIC  ` are the same column. A heading that is not understood is reported
rather than ignored, since a typo there would otherwise drop a whole column
without saying so.

These aliases also work: `task` and `summary` for `title`, `feature` for `epic`,
`details` and `notes` for `description`.

## Agents are not a column

Which agent implements a ticket is decided from what the ticket asks for, so
there is nothing to fill in. A row about tests goes to the Tester, one about a
crash to the Debugger, one about documentation to the Documenter, and anything
else to the Implementer.

That decision reads the title and the description together, which is another
reason to write a real description: `Fix the crash on an empty query` routes
better than `Search fix`.

Nothing is created until you confirm: the import previews the file first and
tells you which rows it cannot use, and why.

---

# Test documentation

`test-report.example.json` is a filled-in example of the file the **Tester**
agent writes at `.opcode/test-report.json` after a test pass. opcode turns that
file into an Excel workbook — **Test doc** in the agent-run header or on the
task board, and `Save an example report` inside that dialog hands you this
example to copy.

Nothing in the app writes the report: the agent does. The app only reads it, so
you can also write or edit it by hand and export the result.

## Why JSON and not a sheet

A ticket import goes *into* opcode from a sheet somebody typed, so a sheet is
the natural input. This goes the other way: the report is written by an agent,
and the steps of a test case are a list, not a cell. JSON holds that shape;
the workbook is what it is rendered into for reading and filtering.

## Sheets in the export

| Sheet | From | What it is for |
| --- | --- | --- |
| Summary | the top-level fields, plus tallies | Scope, commands run, pass rate, and what the report itself is missing |
| Test Cases | `cases` | One row per scenario, filterable by area, category, platform and status |
| Mobile Matrix | `mobile_checks` | One check repeated across devices, orientations and networks |
| Defects | `defects` | What was found, with a reproduction |
| Coverage | `coverage` | Requirement → case ids, so gaps are visible |
| Environments | `environments` | Where it was run: real device, emulator, or browser emulation |

Every sheet is present even when it has no rows. An empty **Defects** tab says
"nothing was found"; a missing one leaves you wondering whether anything was
looked for.

## The fields

Top level: `feature`, `summary`, `tested_by`, `generated_at`, `branch`,
`commit`, `test_commands`, `environments`, `cases`, `mobile_checks`, `defects`,
`coverage`, `not_covered`. All optional — a report with only `feature` and
`cases` exports fine.

A case: `id`, `area`, `title`, `category`, `priority`, `platform`,
`preconditions`, `steps`, `test_data`, `expected`, `actual`, `status`,
`severity`, `automated`, `test_ref`, `defect_id`, `notes`.

`status` (and a matrix row's `result`) is read loosely: `pass`, `Passed`,
`PASS ✅` and `ok` all colour green, and the workbook stores the one canonical
word so the column filters. A status nobody recognises keeps its own text, is
counted as *not run*, and is named in the warnings.

`steps` may be one string or a list of them; a list is numbered in the cell.
`automated` may be `true`, `"yes"` or `"manual"`. Several fields accept the
alternative name an agent is likely to reach for instead — `type` for
`category`, `result` for `status`, `expected_result` for `expected`,
`test_cases` for `cases`, `bugs` for `defects`.

## What gets flagged

The export is never blocked, but the dialog and the Summary sheet both name
what is wrong with the report:

- no test cases, or no feature name
- a case with no `id`, or an `id` used twice
- a case with no `expected`, which cannot pass or fail
- a status that was not understood
- a defect pointing at a case that is not in the report
- **nothing mobile checked** — no case names a mobile platform and the device
  matrix is empty
- **only happy paths** — no edge, boundary or negative cases
- a failure with no defect raised

Read those as review notes on the test pass, not on the wording.

## On a phone

The desktop app saves the workbook through a native dialog. `opcode-web`, which
serves the UI to a phone on your LAN, has no filesystem to write into, so there
the same workbook comes back over HTTP:

```
GET /api/test-report/download?project_path=/path/to/repo
GET /api/test-report            # the same counts and warnings, as JSON
```
