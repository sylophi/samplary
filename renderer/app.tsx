import { FolderPlus } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Inspector } from "@/components/inspector";
import { Results } from "@/components/results";
import { type CategoryFilter, type Place, Sidebar } from "@/components/sidebar";
import { SimilarBar } from "@/components/similar-bar";
import { Toolbar } from "@/components/toolbar";
import {
  api,
  isGuest,
  type Folder,
  type Kind,
  type Overview,
  type Query,
  type Row,
  type SortKey,
  type Weights,
} from "@/lib/api";
import { player } from "@/lib/audio";
import { isOutgoing } from "@/lib/drag";
import { useSearch } from "@/lib/search";
import { clearThumbnails } from "@/lib/waveform";

async function addFolders() {
  const paths = await api.chooseFolders();
  if (paths.length > 0) await api.addFolders(paths);
}

export function App() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [scanning, setScanning] = useState(false);
  const [place, setPlace] = useState<Place>({ folder: null, dir: null, favorites: false });
  const [category, setCategory] = useState<CategoryFilter>({ name: null, subcategory: null });
  const [kind, setKind] = useState<Kind | null>(null);
  const [text, setText] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; descending: boolean; seed: number }>({
    key: "name",
    descending: false,
    seed: 0,
  });
  const [reference, setReference] = useState<Row | null>(null);
  const [weights, setWeights] = useState<Weights>([1, 1, 1, 1]);
  const [selected, setSelected] = useState<Row | null>(null);
  const [dropping, setDropping] = useState(false);
  const [moves, setMoves] = useState(0);
  const searchInput = useRef<HTMLInputElement>(null);
  const textBeforeSimilar = useRef("");

  // The keyboard handler and the stable callbacks below read through this.
  const latest = useRef({
    results: { total: 0, rows: [] as readonly (Row | undefined)[] },
    selected,
    reference,
    text,
  });

  const findSimilar = useCallback((row: Row) => {
    if (!latest.current.reference) textBeforeSimilar.current = latest.current.text;
    setReference(row);
    setSelected(row);
    setText("");
  }, []);

  useEffect(() => api.watchOverview(setOverview), []);

  useEffect(() => {
    let analyzing = false;
    return api.watchProgress((progress) => {
      // Files a scan changed may sound different now.
      if (analyzing && progress?.stage !== "analyzing") {
        clearThumbnails();
        player.forget();
      }
      analyzing = progress?.stage === "analyzing";
      setScanning(progress !== null);
    });
  }, []);

  useEffect(() => {
    // Enter and leave fire for every element passed over on the way.
    let depth = 0;
    const enter = (e: DragEvent) => {
      if (isOutgoing() || !e.dataTransfer?.types.includes("Files")) return;
      depth++;
      setDropping(true);
    };
    const leave = () => {
      if (depth > 0 && --depth === 0) setDropping(false);
    };
    const over = (e: DragEvent) => e.preventDefault();
    const drop = (e: DragEvent) => {
      e.preventDefault();
      depth = 0;
      setDropping(false);
      if (isOutgoing()) return;
      const paths = [...(e.dataTransfer?.files ?? [])].map(api.pathForFile).filter(Boolean);
      if (paths.length === 0) return;
      api
        .dropPaths(paths)
        .then((row) => row && findSimilar(row))
        .catch((error: unknown) =>
          api.alert(
            "That sound can't be read",
            error instanceof Error ? error.message : String(error),
          ),
        );
    };
    window.addEventListener("dragenter", enter);
    window.addEventListener("dragleave", leave);
    window.addEventListener("dragover", over);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", enter);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("dragover", over);
      window.removeEventListener("drop", drop);
    };
  }, [findSimilar]);

  const query: Query = {
    text,
    folder: place.folder,
    dir: place.dir,
    category: category.name,
    subcategory: category.subcategory,
    kind,
    favorites: place.favorites,
    similar: reference?.id ?? null,
    weights,
    sort: sort.key,
    descending: sort.descending,
    seed: sort.seed,
  };
  const queryKey = JSON.stringify(query);
  const { results, fetch, view, refresh } = useSearch(queryKey, overview?.version);
  latest.current = { results, selected, reference, text };

  // What's known about the selection changes as the library does.
  useEffect(() => {
    setSelected((s) => (s && results.rows.find((r) => r?.id === s.id)) ?? s);
  }, [results]);

  const press = useCallback((row: Row) => {
    setSelected(row);
    void player.play(row.id);
  }, []);

  const move = (delta: number) => {
    const { results, selected } = latest.current;
    if (results.total === 0) return;
    const i = selected ? results.rows.findIndex((r) => r?.id === selected.id) : -1;
    const target =
      i < 0
        ? delta > 0
          ? 0
          : results.total - 1
        : Math.min(results.total - 1, Math.max(0, i + delta));
    if (target === i) return;
    const pick = (row: Row) => {
      setSelected(row);
      setMoves((n) => n + 1);
      void player.play(row.id);
    };
    const row = results.rows[target];
    if (row) pick(row);
    else void fetch(target, target + 1).then((rows) => rows.has(target) && pick(rows.get(target)!));
  };

  const exitSimilar = () => {
    setReference(null);
    setText(textBeforeSimilar.current);
  };

  // The selection and the reference show what changed about a row.
  const show = useCallback((id: number, update: (row: Row) => Row) => {
    const apply = (r: Row | null) => (r?.id === id ? update(r) : r);
    setSelected(apply);
    setReference(apply);
  }, []);

  const toggleFavorite = useCallback(
    (row: Row) => {
      if (isGuest(row)) return;
      const favorite = !row.favorite;
      void api.setFavorite(row.id, favorite).then(() => {
        // Among the favorites it comes or goes, moving the pages after it.
        refresh();
        show(row.id, (r) => ({ ...r, favorite }));
      });
    },
    [refresh, show],
  );

  const chooseCategory = useCallback(
    (row: Row, x: number, y: number) => {
      void api.chooseCategory(row, x, y).then((now) => now && show(now.id, () => now));
    },
    [show],
  );

  const removeFolder = async (folder: Folder) => {
    const sure = await api.confirm(
      `Remove “${folder.name}” from the library?`,
      "The files stay where they are.",
      "Remove",
    );
    if (!sure) return;
    await api.removeFolder(folder.id);
  };

  // A folder can leave the library while it's open: removed, or absorbed
  // into a parent folder added after it.
  useEffect(() => {
    if (overview && place.folder !== null && !overview.folders.some((f) => f.id === place.folder)) {
      setPlace({ folder: null, dir: null, favorites: false });
    }
  }, [overview, place.folder]);

  /** A new order each time. */
  const shuffle = () =>
    setSort({ key: "shuffle", descending: false, seed: Math.floor(Math.random() * 2 ** 32) });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      const typing = target instanceof HTMLInputElement && target.type === "text";
      const { selected, reference, text } = latest.current;
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        searchInput.current?.focus();
        searchInput.current?.select();
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        move(e.key === "ArrowDown" ? 1 : -1);
        return;
      }
      if (e.key === "Escape") {
        if (reference) exitSimilar();
        else if (typing && text) setText("");
        else (document.activeElement as HTMLElement | null)?.blur();
        return;
      }
      if (typing) {
        if (e.key === "Enter") target.blur();
        return;
      }
      if (e.key === " ") {
        e.preventDefault();
        if (player.playing) player.stop();
        else if (selected) void player.play(selected.id);
      } else if (e.key === "Enter" && selected) {
        void player.play(selected.id);
      } else if (e.key === "s" && selected) {
        findSimilar(selected);
      } else if (e.key === "f" && selected) {
        toggleFavorite(selected);
      } else if (e.key === "r" && !reference) {
        shuffle();
      } else if (e.key === "/") {
        e.preventDefault();
        searchInput.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const empty = !overview ? null : overview.folders.length === 0 ? (
    <div className="flex h-full flex-col items-center justify-center gap-4 text-[13px] text-muted-foreground">
      <p>Add a folder of samples to start listening.</p>
      <button
        onClick={() => void addFolders()}
        className="flex h-8 items-center gap-2 rounded-md border px-3 text-foreground hover:bg-accent"
      >
        <FolderPlus className="size-4" />
        add folder
      </button>
      <p className="text-xs">or drop folders onto this window</p>
    </div>
  ) : (
    <div className="flex h-full items-center justify-center text-[13px] text-muted-foreground">
      {scanning && overview.total === 0
        ? "Listening to your samples…"
        : reference
          ? "Nothing else here to compare with."
          : "No samples match."}
    </div>
  );

  return (
    <div className="flex h-full">
      <Sidebar
        overview={overview}
        place={place}
        category={category}
        onPlace={setPlace}
        onCategory={setCategory}
        onAddFolder={() => void addFolders()}
        onRemoveFolder={(f) => void removeFolder(f)}
      />
      <main className="relative flex min-w-0 flex-1 flex-col">
        <Toolbar text={text} onText={setText} inputRef={searchInput} kind={kind} onKind={setKind} />
        {reference && (
          <SimilarBar
            reference={reference}
            weights={weights}
            onWeights={setWeights}
            onSelect={() => press(reference)}
            onClose={exitSimilar}
          />
        )}
        <Results
          rows={results.rows}
          onView={view}
          searchKey={queryKey}
          selectedId={selected?.id ?? null}
          moves={moves}
          similar={!!reference}
          sort={sort.key}
          descending={sort.descending}
          onSort={(key) =>
            setSort((s) => ({ ...s, key, descending: s.key === key ? !s.descending : false }))
          }
          onShuffle={shuffle}
          onPress={press}
          onFindSimilar={findSimilar}
          onToggleFavorite={toggleFavorite}
          empty={empty}
        />
        <Inspector
          row={selected}
          onFindSimilar={findSimilar}
          onToggleFavorite={toggleFavorite}
          onChooseCategory={chooseCategory}
        />
        {dropping && (
          <div className="pointer-events-none absolute inset-3 flex items-center justify-center rounded-xl border-2 border-dashed border-ring bg-background/80 text-[13px] text-muted-foreground">
            Drop folders to add them, or a sound to find ones like it
          </div>
        )}
      </main>
    </div>
  );
}
