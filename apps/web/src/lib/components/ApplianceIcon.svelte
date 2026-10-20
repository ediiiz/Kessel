<script lang="ts">
  /**
   * The pictogram for a role class.
   *
   * Kept out of `$lib/appliance.ts` because the values are Svelte components rather than data, and a
   * `.ts` module that imports 8 icon components would pull all of them into every bundle that wanted
   * one string from it.
   *
   * Roles that Lucide has no glyph for get the nearest thing that still reads as the *appliance*,
   * and each substitution is noted below. Two roles an owner has to tell apart in a device list
   * (a dryer beside a washer, a wine cooler beside a fridge) should not share a glyph.
   *
   * An unrecognised role falls back to `CookingPot`, which reads as "appliance" without claiming
   * anything specific.
   */
  import {
    AirVent,
    Coffee,
    CookingPot,
    Droplets,
    Flame,
    Microwave,
    Refrigerator,
    Shirt,
    Snowflake,
    Thermometer,
    WashingMachine,
    Wine,
  } from '$lib/icons';

  const { role, class: className = '' }: { role: string; class?: string } = $props();

  const ICONS: Record<string, typeof CookingPot> = {
    cooktop: Flame,
    hob: Flame,
    // No oven glyph in the set; `microwave` is the closest box-with-a-door.
    oven: Microwave,
    dishwasher: Droplets,
    fridgefreezer: Refrigerator,
    refrigerator: Refrigerator,
    // A freezer and a fridge are the same cabinet in Lucide, so the freezer takes the cold.
    freezer: Snowflake,
    winecooler: Wine,
    washer: WashingMachine,
    washingmachine: WashingMachine,
    // No dryer glyph; `shirt` keeps it in the laundry pair without repeating the washer's drum.
    dryer: Shirt,
    hood: AirVent,
    coffeemaker: Coffee,
    coffeemachine: Coffee,
    warmingdrawer: Thermometer,
    dishwarmer: Thermometer,
  };

  const Glyph = $derived(ICONS[role.toLowerCase()] ?? CookingPot);
</script>

<Glyph class={className} aria-hidden="true" />
