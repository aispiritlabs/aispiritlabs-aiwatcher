# Your own notebook: from data import to saving

In **Data Curation → Pipeline**, switch to **Notebook view** to see an existing
flow as sequential cells. **New notebook flow** creates a new flow with data import,
PHP preparation, a separate Python file, and a dataset save step.
Canvas and notebook views edit the same definition; saving and `.flow.json`
import/export work in both views. Notebook view is available for a connected chain.

The screenshots below show an actual test: importing 12 Titanic passengers,
limiting the result to 3 records in PHP, applying a custom Python transformation,
and publishing the result.

## 1. Import and prepare data

In the first cell, select the source and its parameters. This example uses
`hub_rows`, dataset `phihung/titanic`, split `train`, and limit `12`.
The cell also shows the generated `read(...)` code.

In **Prepare data**, edit the PHP code:

```php
->withEntry('PassengerId', array_get(ref('row'), 'PassengerId'))
->withEntry('Name', array_get(ref('row'), 'Name'))
->select(ref('PassengerId'), ref('Name'))
->limit(3)
```

**Run to here** runs the cells from the source up to the selected step. The output
below each cell comes from that specific step: the import has 12 records, PHP has
3, and Python has not run yet. You can add more blocks from the palette.
A new PHP block goes before the first Python block, and a new Python block goes
before the save step.

![An editable PHP cell with its own output](screenshots/12-notebook-php.png)

## 2. Custom Python code

A new Python cell contains a working template that receives `rows` and `params`
from the previous block. Edit the transformation, keeping the result in `output`:

```python
output = [dict(row, reviewed=True) for row in rows]
```

**Save notebook** saves the code and updates the pinned revision for that cell.
**Save as copy** saves the current code under a new, independent name and switches
the cell to the copy. **New notebook** creates an empty template; the **Python code**
button in the palette adds a new block with its own file.

Unsaved code must be saved or discarded with **Discard code edits** before running
the flow, switching views, or exporting. When opening a saved flow, the editor
shows the exact pinned revision. Saving the flow alone does not replace it with
newer code from the shared file. The optional **Open live output** opens the current
notebook file, which may be newer than the pinned revision; the cell's table always
reflects the completed execution.

![Custom Python code and the added reviewed column](screenshots/13-notebook-python.png)

## 3. Run and save

**Preview 25 rows** checks the entire notebook on a small sample. **Run** processes
the full range of the selected source. Cells show the number of returned records,
up to 25 preview rows, and printed reports. Expanding **Inspect complete values**
shows the full values from those 25 rows.

Set the target name in the **Save dataset** cell. After running the entire flow,
click **Publish dataset**. This action saves the flow definition and a dataset
version; previewing and **Run** alone do not publish data. The test in the screenshot
published 3 records with `PassengerId`, `Name`, and `reviewed`.

**Save** preserves the definition for further work, while **Export flow with code**
transfers blocks, parameters, layout, and PHP/Python sources to a `.flow.json` file.
After refreshing the page or changing the code, run the cells again: preview results
are not part of the saved definition. Changing the code, parameters, connections,
or time range clears previous results and prevents them from being published.

![Dataset save confirmation after running the notebook](screenshots/14-notebook-save.png)

PHP cell previews run successive queries from the source up to each step to
preserve FlowAI aggregation and fitting semantics. For a large flow, this means
more source reads than a normal canvas execution. These reads do not share a
snapshot of a changing source. Python cells run sequentially on the output of the
last PHP block. An error stops subsequent cells and preserves earlier results.

[Back to the full Titanic example](WALKTHROUGH.md).

## Solutions library

Open **Public solutions library**. Enter a term such as `encoder`: the server searches
solutions by title, description, tags, and block kind. **View code and settings**
shows PHP code or parameters; **Python source** fetches the pinned version of the
Python code. The catalogue comes from the API and seed, not a list in the panel code.

![Searching for encoders and previewing code](screenshots/15-solutions-library.png)

Select **Add copy**. The block is added to the flow and can be edited in **Notebook view**.
A Python notebook gets its own file. In the test below, the copy of `Python code`
processed three rows received from the previous cell. **Run to here** shows the
output after the added step.

![An independent block copy and its execution result](screenshots/16-library-copy.png)

In **Canvas view**, select a block, open the library, and expand
**Share selected block**. After entering a description, **Publish solution** shares
the solution with other users of this installation. Saving requires the editor role;
**Add copy** alone does not publish any changes.

![Saving a custom solution through the library API](screenshots/17-library-publish.png)
