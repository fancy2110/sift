<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { SVGAttributes } from 'svelte/elements';

  let {
    name,
    size = 16,
    stroke = 1.7,
    children,
    ...rest
  }: {
    name: string;
    size?: number;
    stroke?: number;
    children?: Snippet;
  } & Omit<SVGAttributes<SVGSVGElement>, 'name' | 'size' | 'stroke'> = $props();

  const paths: Record<string, string> = {
    dashboard:
      'M3.5 3.5h7v7h-7zM13.5 3.5h7v4h-7zM13.5 11h7v9.5h-7zM3.5 14h7v6.5h-7z',
    spark:
      'M12 3l1.7 5.1L19 9.8l-5.3 1.7L12 16.8l-1.7-5.3L5 9.8l5.3-1.7zM18.5 15.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z',
    map: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 3v18M3 12h18M5.6 5.6c3.4 3 9.4 3 12.8 0M5.6 18.4c3.4-3 9.4-3 12.8 0',
    routine:
      'M20 12a8 8 0 1 1-2.34-5.66M20 4v4h-4M12 8v4l3 2',
    queue:
      'M11 6H4M11 12H4M11 18H4M15 8l2 2 4-4M15 14l2 2 4-4',
    settings:
      'M12 9.2a2.8 2.8 0 1 0 0 5.6 2.8 2.8 0 0 0 0-5.6zM19.4 12a7.4 7.4 0 0 0-.12-1.34l2.02-1.57-2-3.46-2.38.96a7.5 7.5 0 0 0-2.32-1.34L14.25 2.5h-4l-.35 2.75a7.5 7.5 0 0 0-2.32 1.34l-2.38-.96-2 3.46 2.02 1.57a7.4 7.4 0 0 0 0 2.68l-2.02 1.57 2 3.46 2.38-.96c.67.58 1.45 1.03 2.32 1.34l.35 2.75h4l.35-2.75a7.5 7.5 0 0 0 2.32-1.34l2.38.96 2-3.46-2.02-1.57c.08-.44.12-.88.12-1.34z',
    chevronRight: 'M9 5l7 7-7 7',
    chevronLeft: 'M15 5l-7 7 7 7',
    chevronUp: 'M6 15l6-6 6 6',
    check: 'M5 12.5l4.5 4.5L19 7.5',
    x: 'M6 6l12 12M18 6L6 18',
    shield: 'M12 3l7 3v5c0 4.6-3 8.4-7 10-4-1.6-7-5.4-7-10V6z M9.5 12l1.8 1.8L15 10',
    bolt: 'M13 3L4 13.5h6L11 21l9-10.5h-6z',
    brain:
      'M9.5 4.5A2.5 2.5 0 0 0 7 7a2.5 2.5 0 0 0-1.5 4.3A2.6 2.6 0 0 0 6 16.5 2.5 2.5 0 0 0 9.5 19a2 2 0 0 0 2.5-2V6.5A2 2 0 0 0 9.5 4.5zM14.5 4.5A2.5 2.5 0 0 1 17 7a2.5 2.5 0 0 1 1.5 4.3 2.6 2.6 0 0 1-1.5 5.2A2.5 2.5 0 0 1 14.5 19a2 2 0 0 1-2.5-2',
    file:
      'M6 2.5h7.5L20 9v11a1.5 1.5 0 0 1-1.5 1.5h-12A1.5 1.5 0 0 1 5 20V4a1.5 1.5 0 0 1 1-1.5zM13.5 3V9H20',
    folder:
      'M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2.5h9A1.5 1.5 0 0 1 21 9v9.5A1.5 1.5 0 0 1 19.5 20h-15A1.5 1.5 0 0 1 3 18.5z',
    trash: 'M4 7h16M9 7V5h6v2M6 7l1 13h10l1-13',
    search: 'M10.5 18a7.5 7.5 0 1 0 0-15 7.5 7.5 0 0 0 0 15zM16 16l4.5 4.5',
    undo: 'M9 7L4 12l5 5M4 12h10a6 6 0 0 1 0 12h-3',
    lock: 'M6 10h12v10H6zM8 10V7a4 4 0 0 1 8 0v3',
    alert: 'M12 4L2.5 20h19zM12 10v5M12 18v.5',
    info: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 7.8v.2',
    arrowRight: 'M5 12h14M13 6l6 6-6 6',
    pause: 'M7 5h3.5v14H7zM13.5 5H17v14h-3.5z',
    play: 'M8 5.5v13l11-6.5z',
    layers: 'M12 3l9 5-9 5-9-5zM3 13l9 5 9-5',
    refresh: 'M20 12a8 8 0 1 1-2.34-5.66M20 4v4h-4',
    hardDrive:
      'M4 6h16v12H4zM4 14h16M7.5 17h.5',
    externalDrive:
      'M5 7h14v9H5zM9 16v2.5M15 16v2.5M8.5 11.5h7',
    clock: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7.5V12l3 2',
    sliders: 'M4 7h10M18 7h2M4 17h2M10 17h10M14 4v6M8 14v6',
    filter: 'M4 5h16l-6 7v6l-4 2v-8z',
    wand: 'M5 15l9-9 4 4-9 9-5 1zM14 5l1 1M18 9l1 1M4 20l1 1',
    home: 'M4 11l8-7 8 7M6 10v9.5h12V10',
    download: 'M12 4v10M7.5 10.5L12 15l4.5-4.5M5 19.5h14',
    desktop: 'M4 5h16v10H4zM10 19h4M8.5 15v4M15.5 15v4',
    film: 'M4 5h16v14H4zM8 5v14M16 5v14M4 9.5h4M16 9.5h4M4 14.5h4M16 14.5h4',
    list: 'M7 6.5h13M7 12h13M7 17.5h13M3.8 6.5h.01M3.8 12h.01M3.8 17.5h.01',
    panelRight:
      'M4 5h16v14H4zM15 5v14M8 9.5l3 2.5-3 2.5',
    folderPlus: 'M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2.5h9A1.5 1.5 0 0 1 21 9v9.5A1.5 1.5 0 0 1 19.5 20h-15A1.5 1.5 0 0 1 3 18.5zM12 10v5M9.5 12.5h5'
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  {...rest}
>
  <path d={paths[name] ?? ''}></path>
</svg>
{@render children?.()}
