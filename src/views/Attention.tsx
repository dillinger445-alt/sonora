import { useId, useMemo } from 'react';
import { LuCircleCheck } from 'react-icons/lu';
import type { LibraryFilter } from '../api/types.ts';
import { TabList, tabId } from '../components/Controls.tsx';
import { EmptyState, ViewHeader } from '../components/Layout.tsx';
import { navigate } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { useResource } from '../state/useResource.ts';
import { LibraryBrowser } from './LibraryBrowser.tsx';

interface Category {
  id: string;
  label: string;
  description: string;
  filter: LibraryFilter;
  count: (c: { flag: (id: string) => number; failed: number; missing: number; attention: number }) => number;
}

const CATEGORIES: Category[] = [
  {
    id: 'all',
    label: 'All',
    description: 'Every track with a significant problem, a failed analysis or a missing file.',
    filter: { attention: true },
    count: (c) => c.attention,
  },
  {
    id: 'clipping',
    label: 'Clipping',
    description: 'Clipped samples or decoded overs: audible distortion on loud passages, worst on club systems.',
    filter: { flags: ['clipping'] },
    count: (c) => c.flag('clipping'),
  },
  {
    id: 'source_concern',
    label: 'Lossy source',
    description: 'Signs that the audio passed through a lossy encoder before this file was made, such as an MP3 inside a FLAC.',
    filter: { flags: ['source_concern'] },
    count: (c) => c.flag('source_concern'),
  },
  {
    id: 'low_bitrate',
    label: 'Low bitrate',
    description: 'Lossy files encoded at bitrates where compression artifacts are usually audible. Look for a better source.',
    filter: { flags: ['low_bitrate'] },
    count: (c) => c.flag('low_bitrate'),
  },
  {
    id: 'integrity',
    label: 'Integrity',
    description: 'Truncated files, decode errors, checksum mismatches, dropouts and abrupt endings.',
    filter: { flags: ['integrity'] },
    count: (c) => c.flag('integrity'),
  },
  {
    id: 'review',
    label: 'Worth a listen',
    description: 'Possible problems backed only by weak evidence. Listen before acting on them.',
    filter: { flags: ['review'] },
    count: (c) => c.flag('review'),
  },
  {
    id: 'failed',
    label: 'Failed',
    description: 'Files that could not be decoded or analyzed. The reason is shown in the Issues column.',
    filter: { statuses: ['failed'] },
    count: (c) => c.failed,
  },
  {
    id: 'missing',
    label: 'Missing',
    description: 'Files no longer at their recorded location. Rescanning their folder relinks moved or renamed files.',
    filter: { statuses: ['missing'] },
    count: (c) => c.missing,
  },
];

export function AttentionView({ flag }: { flag?: string }) {
  const { client, overview } = useCore();
  const idBase = useId();
  const facets = useResource(() => client.facets(), [], { events: ['library.changed', 'scan.finished', 'scan.item'], throttleMs: 3000 });
  const current = CATEGORIES.find((c) => c.id === flag) ?? (CATEGORIES[0] as Category);
  const counts = useMemo(() => {
    const f = facets.data;
    return {
      flag: (id: string) => f?.flags.find((x) => x.id === id)?.count ?? 0,
      failed: f?.statuses.find((s) => s.id === 'failed')?.count ?? 0,
      missing: f?.missing ?? 0,
      attention: overview?.attention ?? 0,
    };
  }, [facets.data, overview?.attention]);

  return (
    <div className="view view-fill">
      <ViewHeader title="Needs attention" subtitle={current.description} />
      <TabList
        variant="underline"
        label="Problem category"
        idBase={idBase}
        panelId={`${idBase}-panel`}
        value={current.id}
        items={CATEGORIES.map((c) => ({ id: c.id, label: c.label, count: c.count(counts) }))}
        onChange={(id) => navigate({ view: 'attention', flag: id === 'all' ? undefined : id }, true)}
      />
      <LibraryBrowser
        key={current.id}
        prefsKey="attention"
        label={`Needs attention: ${current.label}`}
        baseFilter={current.filter}
        defaultSort={{ key: 'overall', desc: false }}
        showIssueFilter={false}
        panel={{ id: `${idBase}-panel`, labelledBy: tabId(idBase, current.id) }}
        emptyBase={
          <EmptyState icon={LuCircleCheck} title={current.id === 'all' ? 'Nothing needs attention' : `No tracks in “${current.label}”`}>
            {overview && overview.tracks.analyzed === 0
              ? 'Problems appear here once tracks have been analyzed.'
              : 'Analyzed tracks with this kind of problem will be listed here.'}
          </EmptyState>
        }
      />
    </div>
  );
}
