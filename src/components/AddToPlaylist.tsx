/** "Add to playlist" for chosen tracks: any playlist, or a new one made of them. */

import { LuListMusic, LuListPlus } from 'react-icons/lu';
import { plural } from '../lib/format.ts';
import { usePlaylists } from '../state/playlists.tsx';
import { Button } from './Button.tsx';
import { Menu, type MenuItem } from './Menu.tsx';

export function AddToPlaylist({ ids, size = 'md', variant = 'secondary', label = 'Add to playlist' }: { ids: number[]; size?: 'sm' | 'md'; variant?: 'secondary' | 'ghost'; label?: string }) {
  const pl = usePlaylists();
  const targets = pl.nodes
    .filter((n) => n.kind === 'playlist')
    .map((n) => ({ n, path: pl.pathOf(n.id) }))
    .sort((a, b) => `${a.path}/${a.n.name}`.localeCompare(`${b.path}/${b.n.name}`, undefined, { sensitivity: 'base' }));
  const items: MenuItem[] = [
    {
      id: 'new',
      label: ids.length === 1 ? 'New playlist with this track' : `New playlist of ${plural(ids.length, 'track')}`,
      icon: LuListPlus,
      onSelect: () => void pl.create('playlist', { trackIds: ids, open: false }),
    },
    ...(targets.length ? [{ kind: 'separator', id: 'sep' } as MenuItem] : []),
    ...targets.slice(0, 200).map(
      ({ n, path }): MenuItem => ({
        id: `p${n.id}`,
        label: path ? `${path} / ${n.name}` : n.name,
        icon: LuListMusic,
        hint: n.count ? String(n.count) : undefined,
        onSelect: () => void pl.addTracks(n.id, ids),
      }),
    ),
  ];
  return (
    <Menu
      label={label}
      items={items}
      trigger={(p) => (
        <Button size={size} variant={variant} icon={LuListPlus} disabled={ids.length === 0} {...p}>
          {label}
        </Button>
      )}
    />
  );
}
