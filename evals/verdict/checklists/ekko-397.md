1. **Stashed filter.** A stashed filter exists and search honors it. With the filter on, search returns stashed items that hold every word of the query. The query 'Tab completion ids projects phases' finds stashed task 107.
2. **Line when nothing is found.** Without the filter, search results still leave stashed items out. But when such a search finds nothing, it adds a line saying how many stashed items match. The SEEN query now reports no live match plus a stashed count that includes task 107. The fix does not just drop the exclusion.
3. **Count uses the search's matching.** The stashed count uses the same matching as the search itself: every word, the same fields and the same other filters in effect. It equals what the stashed filter would return for that query, not the board's total of 82 stashed items.
4. **MCP as well as CLI.** Sessions get both the filter and the line through MCP, not only through the CLI. The MCP search tool accepts the stashed filter, visibly in its schema or description, and its reply carries the stashed-count line.
5. **Line when little is found.** The line also appears when a search finds only a few items, not only when it finds none. The cutoff for "few" is the implementer's choice. (open)
6. **Works with existing filters.** The stashed filter combines with the existing filters wherever those are accepted, for example only stashed notes, or stashed items of one project. It is not a separate path that ignores them.
7. **Test for the SEEN case.** A test covers the SEEN case: a stashed item holding every word of a query:
   - is absent from default results,
   - is counted in the line,
   - is returned when the filter is on.
8. **Free choices.** These are left to the implementer (open):
   - the filter's name and syntax;
   - whether it returns only stashed items or stashed plus live ones;
   - how stashed hits are marked;
   - the line's wording (naming the filter to use is a plus);
   - whether the line appears when no stashed item matches.
