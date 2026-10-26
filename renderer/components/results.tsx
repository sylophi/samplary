import { ArrowDown, ArrowUp, ScanSearch, Shuffle, Star } from "lucide-react";
import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";

import { WaveCanvas } from "@/components/wave-canvas";
import { drawnSeconds, type Row, type SortKey } from "@/lib/api";
import { player, usePlaying } from "@/lib/audio";
import { dragOut, pressOrDrag } from "@/lib/drag";
import { formatDuration, formatTempo, pitchLabel, shortFolder } from "@/lib/format";
import { useDark, useThumbnail } from "@/lib/waveform";

const ROW_HEIGHT = 36;
const OVERSCAN = 12;
const THUMB_WIDTH = 112;
const THUMB_HEIGHT = 24;

const WIDTHS_KEY = "samplary-columns";

/**
 * Widths of the columns after the name, which takes what's left. In a
 * window too narrow for them all, they give up room in proportion.
 */
type Widths = Record<"folder" | "category" | "pitch" | "bpm" | "length", number>;
type Column = keyof Widths | "name";

const DEFAULT_WIDTHS: Widths = { folder: 220, category: 136, pitch: 48, bpm: 48, length: 64 };
const MIN_WIDTH = 40;
const MIN_NAME_WIDTH = 96;

// Rows size their cells by these variables, so a resize doesn't render them.
const COLUMNS: { key: Column; sort: SortKey; className: string }[] = [
  { key: "name", sort: "name", className: "min-w-24 flex-1" },
  { key: "folder", sort: "folder", className: "min-w-0 basis-(--col-folder)" },
  { key: "category", sort: "category", className: "min-w-0 basis-(--col-category)" },
  { key: "pitch", sort: "pitch", className: "min-w-0 basis-(--col-pitch)" },
  { key: "bpm", sort: "tempo", className: "min-w-0 basis-(--col-bpm) text-right" },
  { key: "length", sort: "duration", className: "min-w-0 basis-(--col-length) text-right" },
];
const cell = Object.fromEntries(COLUMNS.map((c) => [c.key, c.className])) as Record<Column, string>;

function loadWidths(): Widths {
  try {
    return { ...DEFAULT_WIDTHS, ...JSON.parse(localStorage.getItem(WIDTHS_KEY) ?? "{}") };
  } catch {
    return DEFAULT_WIDTHS;
  }
}

/**
 * Drags the line between `left` and `right`: one widens as the other
 * narrows, and the rest stay put.
 */
function resize(
  e: React.MouseEvent<HTMLElement>,
  left: Column,
  right: keyof Widths,
  widths: Widths,
  onWidths: (widths: Widths) => void,
) {
  e.preventDefault();
  // From the widths asked for, not the ones a narrow window squeezed them to.
  // The name takes what's left.
  const leftWidth =
    left === "name"
      ? e.currentTarget.parentElement!.previousElementSibling!.getBoundingClientRect().width
      : widths[left];
  const rightWidth = widths[right];
  const leftMin = left === "name" ? MIN_NAME_WIDTH : MIN_WIDTH;
  const x = e.clientX;
  let next = widths;
  const move = (e: MouseEvent) => {
    const dx = Math.min(Math.max(e.clientX - x, leftMin - leftWidth), rightWidth - MIN_WIDTH);
    next = { ...widths, [right]: rightWidth - dx };
    if (left !== "name") next[left] = leftWidth + dx;
    onWidths(next);
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    document.body.style.cursor = "";
    localStorage.setItem(WIDTHS_KEY, JSON.stringify(next));
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
  document.body.style.cursor = "col-resize";
}

export function Playhead({ id }: { id: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const playing = usePlaying();
  const active = playing?.id === id;
  useEffect(() => {
    if (!active || !playing) return;
    let frame = 0;
    const tick = () => {
      const t = player.position();
      if (ref.current && t !== null) {
        ref.current.style.left = `${Math.min(1, t / drawnSeconds(playing.duration)) * 100}%`;
      }
      frame = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(frame);
  }, [active, playing]);
  if (!active) return null;
  return <div ref={ref} className="pointer-events-none absolute inset-y-0 w-px bg-playhead" />;
}

function Guessed({ guessed, children }: { guessed: boolean; children: React.ReactNode }) {
  return guessed ? (
    <span className="text-muted-foreground italic" title="Guessed from how it sounds">
      {children}
    </span>
  ) : (
    children
  );
}

/** The row's category and its kind, guesses in italics. */
export function CategoryLabel({ row }: { row: Row }) {
  return (
    <span>
      <Guessed guessed={row.categoryGuessed}>{row.category}</Guessed>
      {row.subcategory && (
        <span className="ml-0.5 text-muted-foreground">
          {" "}
          <Guessed guessed={row.subcategoryGuessed}>{row.subcategory}</Guessed>
        </span>
      )}
    </span>
  );
}

function Match({ value }: { value: number }) {
  return (
    <div className="flex w-20 shrink-0 items-center gap-2" title="How alike it sounds">
      <div className="h-1 flex-1 overflow-hidden rounded-full bg-border">
        <div className="h-full rounded-full bg-match" style={{ width: `${value * 100}%` }} />
      </div>
      <span className="w-7 text-right text-xs text-muted-foreground tabular-nums">
        {Math.round(value * 100)}
      </span>
    </div>
  );
}

const ResultRow = memo(function ResultRow({
  row,
  top,
  selected,
  similar,
  onPress,
  onFindSimilar,
  onToggleFavorite,
}: {
  row: Row;
  top: number;
  selected: boolean;
  similar: boolean;
  onPress: (row: Row) => void;
  onFindSimilar: (row: Row) => void;
  onToggleFavorite: (row: Row) => void;
}) {
  const thumb = useThumbnail(row.id);
  const dark = useDark();
  const pitch = pitchLabel(row);
  return (
    <div
      data-row={row.id}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        onPress(row);
        pressOrDrag(e, { onDrag: () => dragOut(row, dark) });
      }}
      className={`group absolute inset-x-0 flex items-center gap-3 px-4 text-[13px] ${
        selected ? "bg-selected" : "hover:bg-accent"
      }`}
      style={{ top, height: ROW_HEIGHT }}
    >
      {similar && row.similarity !== null && <Match value={row.similarity} />}
      <div className="relative shrink-0" style={{ width: THUMB_WIDTH, height: THUMB_HEIGHT }}>
        <WaveCanvas data={thumb} width={THUMB_WIDTH} height={THUMB_HEIGHT} />
        <Playhead id={row.id} />
      </div>
      <div className={`${cell.name} truncate`}>{row.name}</div>
      <div className={`${cell.folder} truncate text-muted-foreground`} title={row.folder}>
        {shortFolder(row.folder)}
      </div>
      <div className={`${cell.category} truncate`}>
        <CategoryLabel row={row} />
      </div>
      <div className={`${cell.pitch} truncate`}>{pitch ?? ""}</div>
      <div className={`${cell.bpm} truncate tabular-nums`}>
        {row.tempo ? formatTempo(row.tempo) : ""}
      </div>
      <div className={`${cell.length} truncate text-muted-foreground tabular-nums`}>
        {formatDuration(row.duration)}
      </div>
      <div className="flex w-12 shrink-0 items-center justify-end gap-1">
        <button
          title="Find similar sounds (S)"
          onMouseDown={(e) => e.stopPropagation()}
          onClick={() => onFindSimilar(row)}
          className="hidden size-5 items-center justify-center rounded text-muted-foreground group-hover:flex hover:text-foreground"
        >
          <ScanSearch className="size-3.5" />
        </button>
        <button
          title={row.favorite ? "Remove from favorites (F)" : "Add to favorites (F)"}
          onMouseDown={(e) => e.stopPropagation()}
          onClick={() => onToggleFavorite(row)}
          className={`size-5 items-center justify-center rounded ${
            row.favorite
              ? "flex text-foreground"
              : "hidden text-muted-foreground group-hover:flex hover:text-foreground"
          }`}
        >
          <Star className={`size-3.5 ${row.favorite ? "fill-current" : ""}`} />
        </button>
      </div>
    </div>
  );
});

export function Results({
  rows,
  onView,
  searchKey,
  selectedId,
  moves,
  similar,
  sort,
  descending,
  onSort,
  onShuffle,
  onPress,
  onFindSimilar,
  onToggleFavorite,
  empty,
}: {
  /** Every result, each row once it has been fetched. */
  rows: readonly (Row | undefined)[];
  /** Told which rows are in view, as they change. */
  onView: (start: number, end: number) => void;
  searchKey: string;
  selectedId: number | null;
  /** Counts keyboard moves of the selection. */
  moves: number;
  similar: boolean;
  sort: SortKey;
  descending: boolean;
  onSort: (key: SortKey) => void;
  onShuffle: () => void;
  onPress: (row: Row) => void;
  onFindSimilar: (row: Row) => void;
  onToggleFavorite: (row: Row) => void;
  empty: React.ReactNode;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(0);
  const [widths, setWidths] = useState(loadWidths);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setHeight(el.clientHeight));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Scrolling back to where it was within one frame fires no scroll event,
  // so the rows to draw follow every scroll made here directly.
  const scrollTo = (el: HTMLDivElement, top: number) => {
    el.scrollTop = top;
    setScrollTop(el.scrollTop);
  };

  // A new search starts at the top.
  useLayoutEffect(() => {
    if (scroller.current) scrollTo(scroller.current, 0);
  }, [searchKey]);

  // Keep the selection in view as the keyboard moves it. Only then: rows
  // that merely change shouldn't pull the list away from where it is.
  useEffect(() => {
    const el = scroller.current;
    const index = rows.findIndex((r) => r?.id === selectedId);
    if (!el || index < 0) return;
    const top = index * ROW_HEIGHT;
    if (top < el.scrollTop) scrollTo(el, top);
    else if (top + ROW_HEIGHT > el.scrollTop + el.clientHeight) {
      scrollTo(el, top + ROW_HEIGHT - el.clientHeight);
    }
  }, [moves]);

  const start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const end = Math.min(rows.length, Math.ceil((scrollTop + height) / ROW_HEIGHT) + OVERSCAN);
  const viewEnd = Math.ceil((scrollTop + height) / ROW_HEIGHT) + OVERSCAN;

  useEffect(() => onView(start, viewEnd), [start, viewEnd, onView]);

  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      style={Object.fromEntries(Object.entries(widths).map(([k, w]) => [`--col-${k}`, `${w}px`]))}
    >
      <div className="group/header flex h-8 shrink-0 items-center gap-3 border-b px-4 text-xs text-muted-foreground">
        {similar && <div className="w-20 shrink-0">match</div>}
        <div className="shrink-0" style={{ width: THUMB_WIDTH }}>
          <button
            title="Shuffle (R)"
            disabled={similar}
            onClick={onShuffle}
            className={`flex items-center gap-1 ${
              !similar && sort === "shuffle" ? "text-foreground" : ""
            } enabled:hover:text-foreground`}
          >
            <Shuffle className="size-3" />
            shuffle
          </button>
        </div>
        {COLUMNS.map((c, i) => (
          <div key={c.key} className={`relative flex h-full ${c.className}`}>
            <button
              disabled={similar}
              onClick={() => onSort(c.sort)}
              className={`flex min-w-0 flex-1 items-center gap-1 ${
                c.className.includes("text-right") ? "justify-end" : ""
              } ${!similar && sort === c.sort ? "text-foreground" : ""} enabled:hover:text-foreground`}
            >
              <span className="truncate">{c.key}</span>
              {!similar &&
                sort === c.sort &&
                (descending ? (
                  <ArrowDown className="size-3 shrink-0" />
                ) : (
                  <ArrowUp className="size-3 shrink-0" />
                ))}
            </button>
            {c.key !== "name" && (
              <div
                onMouseDown={(e) =>
                  resize(e, COLUMNS[i - 1].key, c.key as keyof Widths, widths, setWidths)
                }
                className="absolute inset-y-0 -left-3 flex w-3 cursor-col-resize justify-center py-2"
              >
                <div className="w-px bg-transparent group-hover/header:bg-border" />
              </div>
            )}
          </div>
        ))}
        <div className="w-12 shrink-0" />
      </div>
      <div
        ref={scroller}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        className="relative min-h-0 flex-1 overflow-y-auto"
      >
        {rows.length === 0 ? (
          empty
        ) : (
          <div style={{ height: rows.length * ROW_HEIGHT }}>
            {rows
              .slice(start, end)
              .map((row, i) =>
                row ? (
                  <ResultRow
                    key={row.id}
                    row={row}
                    top={(start + i) * ROW_HEIGHT}
                    selected={row.id === selectedId}
                    similar={similar}
                    onPress={onPress}
                    onFindSimilar={onFindSimilar}
                    onToggleFavorite={onToggleFavorite}
                  />
                ) : (
                  <div
                    key={`pending-${start + i}`}
                    className="absolute inset-x-0"
                    style={{ top: (start + i) * ROW_HEIGHT, height: ROW_HEIGHT }}
                  />
                ),
              )}
          </div>
        )}
      </div>
    </div>
  );
}
