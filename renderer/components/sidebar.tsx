import { ChevronRight, CircleArrowUp, Library, Plus, Star, X } from "lucide-react";
import { useEffect, useState } from "react";

import { api, type Folder, type Overview, type Progress } from "@/lib/api";
import { formatCount } from "@/lib/format";

export interface Place {
  folder: number | null;
  dir: string | null;
  favorites: boolean;
}

export interface CategoryFilter {
  name: string | null;
  subcategory: string | null;
}

interface Node {
  name: string;
  path: string;
  count: number;
  children: Node[];
}

function tree(folder: Folder): Node[] {
  const root: Node = { name: "", path: "", count: 0, children: [] };
  const byPath = new Map<string, Node>([["", root]]);
  // Paths arrive sorted, so every parent comes before its children.
  for (const dir of folder.dirs) {
    const cut = dir.path.lastIndexOf("/");
    const node: Node = {
      name: dir.path.slice(cut + 1),
      path: dir.path,
      count: dir.count,
      children: [],
    };
    (byPath.get(cut < 0 ? "" : dir.path.slice(0, cut)) ?? root).children.push(node);
    byPath.set(dir.path, node);
  }
  const sort = (nodes: Node[]) => {
    nodes.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
    for (const n of nodes) sort(n.children);
  };
  sort(root.children);
  return root.children;
}

function Item({
  depth = 0,
  active,
  label,
  count,
  icon,
  open,
  onToggle,
  onClick,
  trailing,
}: {
  depth?: number;
  active: boolean;
  label: string;
  count?: number;
  icon?: React.ReactNode;
  open?: boolean;
  onToggle?: () => void;
  onClick: () => void;
  trailing?: React.ReactNode;
}) {
  return (
    <div
      onClick={onClick}
      className={`group flex h-7 items-center gap-1.5 rounded-md pr-2 text-[13px] ${
        active ? "bg-selected text-foreground" : "text-foreground/85 hover:bg-accent"
      }`}
      style={{ paddingLeft: 6 + depth * 14 }}
    >
      {onToggle ? (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onToggle();
          }}
          className="flex size-4 shrink-0 items-center justify-center text-muted-foreground"
        >
          <ChevronRight className={`size-3.5 transition-transform ${open ? "rotate-90" : ""}`} />
        </button>
      ) : (
        <span className="w-4 shrink-0">{icon}</span>
      )}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {trailing}
      {count !== undefined && (
        <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
          {formatCount(count)}
        </span>
      )}
    </div>
  );
}

function Heading({ children, actions }: { children: string; actions?: React.ReactNode }) {
  return (
    <div className="mt-5 mb-1 flex h-6 items-center justify-between px-2 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
      {children}
      {actions}
    </div>
  );
}

export function Sidebar({
  overview,
  place,
  category,
  onPlace,
  onCategory,
  onAddFolder,
  onRemoveFolder,
}: {
  overview: Overview | null;
  place: Place;
  category: CategoryFilter;
  onPlace: (place: Place) => void;
  onCategory: (category: CategoryFilter) => void;
  onAddFolder: () => void;
  onRemoveFolder: (folder: Folder) => void;
}) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (key: string) =>
    setOpen((o) => {
      const next = new Set(o);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  const all = !place.favorites && place.folder === null;

  const renderNodes = (folder: Folder, nodes: Node[], depth: number): React.ReactNode =>
    nodes.map((node) => {
      const key = `${folder.id}:${node.path}`;
      return (
        <div key={key}>
          <Item
            depth={depth}
            active={place.folder === folder.id && place.dir === node.path}
            label={node.name}
            count={node.count}
            open={open.has(key)}
            onToggle={node.children.length ? () => toggle(key) : undefined}
            onClick={() => onPlace({ folder: folder.id, dir: node.path, favorites: false })}
          />
          {open.has(key) && renderNodes(folder, node.children, depth + 1)}
        </div>
      );
    });

  return (
    <aside className="flex w-64 shrink-0 flex-col border-r bg-sidebar">
      <div data-drag-region className="h-13 shrink-0" />
      <nav className="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
        <Item
          active={all}
          label="all samples"
          count={overview?.total}
          icon={<Library className="size-3.5 text-muted-foreground" />}
          onClick={() => onPlace({ folder: null, dir: null, favorites: false })}
        />
        <Item
          active={place.favorites}
          label="favorites"
          count={overview?.favorites}
          icon={<Star className="size-3.5 text-muted-foreground" />}
          onClick={() => onPlace({ folder: null, dir: null, favorites: true })}
        />

        <Heading
          actions={
            <button
              title="Add a folder"
              onClick={onAddFolder}
              className="flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <Plus className="size-4" />
            </button>
          }
        >
          folders
        </Heading>
        {overview?.folders.map((folder) => {
          const key = `${folder.id}:`;
          const nodes = tree(folder);
          return (
            <div key={folder.id}>
              <Item
                active={place.folder === folder.id && place.dir === null}
                label={folder.name}
                count={folder.count}
                open={open.has(key)}
                onToggle={nodes.length ? () => toggle(key) : undefined}
                onClick={() => onPlace({ folder: folder.id, dir: null, favorites: false })}
                trailing={
                  <button
                    title="Remove from library"
                    onClick={(e) => {
                      e.stopPropagation();
                      onRemoveFolder(folder);
                    }}
                    className="hidden size-4 shrink-0 items-center justify-center text-muted-foreground group-hover:flex hover:text-foreground"
                  >
                    <X className="size-3.5" />
                  </button>
                }
              />
              {open.has(key) && renderNodes(folder, nodes, 1)}
            </div>
          );
        })}

        {!!overview?.categories.length && (
          <>
            <Heading>categories</Heading>
            {overview.categories.map((c) => {
              const key = `category:${c.name}`;
              const here = category.name === c.name;
              return (
                <div key={c.name}>
                  <Item
                    active={here && category.subcategory === null}
                    label={c.name}
                    count={c.count}
                    open={open.has(key)}
                    onToggle={c.subcategories.length ? () => toggle(key) : undefined}
                    onClick={() => {
                      const leaving = here && category.subcategory === null;
                      onCategory({ name: leaving ? null : c.name, subcategory: null });
                      // Choosing a category shows its subcategories.
                      if (!leaving) setOpen((o) => new Set(o).add(key));
                    }}
                  />
                  {open.has(key) &&
                    c.subcategories.map((s) => (
                      <Item
                        key={s.name}
                        depth={1}
                        active={here && category.subcategory === s.name}
                        label={s.name}
                        count={s.count}
                        onClick={() =>
                          onCategory({
                            name: c.name,
                            subcategory: here && category.subcategory === s.name ? null : s.name,
                          })
                        }
                      />
                    ))}
                </div>
              );
            })}
          </>
        )}
      </nav>
      <ScanProgress />
      <Update />
    </aside>
  );
}

/** A restart away from a newer version, once one has downloaded. */
function Update() {
  const [ready, setReady] = useState(false);
  useEffect(() => api.watchUpdateReady(setReady), []);
  if (!ready) return null;
  return (
    <div className="shrink-0 border-t px-4 py-3">
      <button
        onClick={() => void api.installUpdate()}
        className="flex h-7 w-full items-center justify-center gap-1.5 rounded-md border text-[13px] hover:bg-accent"
      >
        <CircleArrowUp className="size-3.5" />
        restart to update
      </button>
    </div>
  );
}

/** How far the scan has got, while there's one. */
function ScanProgress() {
  const [progress, setProgress] = useState<Progress | null>(null);
  useEffect(() => api.watchProgress(setProgress), []);
  if (!progress) return null;
  return (
    <div className="shrink-0 border-t px-4 py-3 text-xs text-muted-foreground">
      <div className="flex justify-between">
        <span>{progress.stage}</span>
        <span className="tabular-nums">
          {formatCount(progress.done)} / {formatCount(progress.total)}
        </span>
      </div>
      <div className="mt-2 h-0.5 overflow-hidden rounded-full bg-border">
        <div
          className="h-full bg-foreground transition-[width]"
          style={{ width: `${(progress.done / Math.max(progress.total, 1)) * 100}%` }}
        />
      </div>
    </div>
  );
}
