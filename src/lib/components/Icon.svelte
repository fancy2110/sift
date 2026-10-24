<script lang="ts">
  /**
   * Icon facade over Lucide (ISC): one consistent 24px grid, round caps and a
   * 1.7 default stroke, which matches the app's thin-line visual language far
   * better than the hand-drawn paths it replaced.
   *
   * Call sites keep using `<Icon name="trash" size={12} />`; each name maps to
   * a Lucide component here.
   */
  import type { Snippet } from 'svelte';
  import type { SVGAttributes } from 'svelte/elements';
  import {
    Brain,
    Check,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    Clock,
    Filter,
    Gauge,
    HardDrive,
    Layers,
    Pause,
    Play,
    RefreshCw,
    Repeat,
    Settings,
    ShieldCheck,
    SlidersHorizontal,
    Sparkles,
    Trash2,
    Undo2,
    X,
    Zap
  } from 'lucide-svelte';

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

  const icons: Record<string, any> = {
    bolt: Zap,
    brain: Brain,
    check: Check,
    chevronLeft: ChevronLeft,
    chevronRight: ChevronRight,
    chevronUp: ChevronUp,
    clock: Clock,
    filter: Filter,
    gauge: Gauge,
    hardDrive: HardDrive,
    layers: Layers,
    pause: Pause,
    play: Play,
    refresh: RefreshCw,
    routine: Repeat,
    settings: Settings,
    shield: ShieldCheck,
    sliders: SlidersHorizontal,
    spark: Sparkles,
    trash: Trash2,
    undo: Undo2,
    x: X
  };

  const Glyph = $derived(icons[name] as any);
</script>

{#if Glyph}
  <Glyph size={size} strokeWidth={stroke} aria-hidden="true" {...rest} />
{:else}
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
  ></svg>
{/if}
{@render children?.()}
