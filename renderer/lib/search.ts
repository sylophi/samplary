// Searching page by page: the rows in view, and those just around them,
// are fetched as they come into view.
import { useCallback, useEffect, useRef, useState } from "react";

import { api, type Query, type Row } from "@/lib/api";

const PAGE = 100;

export interface Results {
  readonly total: number;
  /** Each result's row, once its page has come. */
  readonly rows: readonly (Row | undefined)[];
}

/**
 * The results of the query in `queryKey` (its JSON), searched again
 * whenever `version` changes.
 */
export function useSearch(queryKey: string, version: number | undefined) {
  const [results, setResults] = useState<Results>({ total: 0, rows: [] });
  const search = useRef({
    queryKey,
    /** Counts the searches, so a late answer to an old one is dropped. */
    generation: 0,
    requested: new Set<number>(),
    /** Whether this search's first answer has replaced the last one's rows. */
    replaced: false,
    /** The rows in view. */
    view: [0, PAGE] as [number, number],
  });

  /** Fetches the rows from `start` to `end` not yet asked for, and hands them back. */
  const fetch = useCallback(async (start: number, end: number) => {
    const s = search.current;
    const generation = s.generation;
    const pages: number[] = [];
    for (let p = Math.floor(start / PAGE); p * PAGE < Math.max(end, start + 1); p++) {
      if (!s.requested.has(p)) pages.push(p);
    }
    for (const p of pages) s.requested.add(p);
    const fetched = new Map<number, Row>();
    if (pages.length === 0) return fetched;
    const query = JSON.parse(s.queryKey) as Query;
    const answers = await Promise.all(pages.map((p) => api.search(query, p * PAGE, PAGE)));
    if (search.current.generation !== generation) return fetched;
    pages.forEach((p, i) => answers[i].rows.forEach((row, j) => fetched.set(p * PAGE + j, row)));
    const replace = !s.replaced;
    s.replaced = true;
    setResults((old) => {
      const total = answers[0].total;
      const rows = replace ? Array.from<Row | undefined>({ length: total }) : old.rows.slice();
      for (const [i, row] of fetched) rows[i] = row;
      return { total, rows };
    });
    return fetched;
  }, []);

  const restart = useCallback(
    (queryKey: string) => {
      const s = search.current;
      const sameQuery = s.queryKey === queryKey;
      s.queryKey = queryKey;
      s.generation++;
      s.requested = new Set();
      s.replaced = false;
      // A changed library is searched again where the list is. A new search
      // starts at the top.
      const [start, end] = sameQuery ? s.view : [0, s.view[1] - s.view[0]];
      void fetch(start, end);
    },
    [fetch],
  );

  useEffect(() => restart(queryKey), [queryKey, version, restart]);

  /** Searches again where the list is, for a change `version` doesn't count. */
  const refresh = useCallback(() => restart(search.current.queryKey), [restart]);

  /**
   * Tells which rows are in view, fetching any not yet there, and a page
   * either side, so a quick scroll finds its rows already come.
   */
  const view = useCallback(
    (start: number, end: number) => {
      search.current.view = [start, end];
      void fetch(Math.max(0, start - PAGE), end + PAGE);
    },
    [fetch],
  );

  return { results, fetch, view, refresh };
}
