import { useEffect, useMemo, useState } from "react";
import type { BundledPet } from "./pet-sprite";

// Runtime-fetched bundled-pet roster (`public/pets/index.json`), cached at
// module scope for the process lifetime — mirrors PetSprite.tsx's
// `dataUrlCache` idiom (fetch once, every hook instance across the whole app
// session reuses the same resolved list instead of re-fetching).
let cachedPets: BundledPet[] | null = null;
let inFlight: Promise<BundledPet[]> | null = null;

async function loadBundledPets(): Promise<BundledPet[]> {
  if (cachedPets !== null) return cachedPets;
  if (inFlight !== null) return inFlight;
  inFlight = (async () => {
    try {
      const response = await fetch("/pets/index.json");
      const pets: BundledPet[] = response.ok ? await response.json() : [];
      cachedPets = pets;
      return pets;
    } catch {
      // Same fail-soft posture as PetSprite's resolveSrc: a missing/broken
      // index just means "no bundled pets known yet", never a thrown error.
      cachedPets = [];
      return cachedPets;
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}

/**
 * Test-only escape hatch: clears the module-scope cache so a fresh fetch
 * scenario (success vs. failure) can be exercised from a clean slate.
 * Never call this from application code.
 */
export function __resetBundledPetsCacheForTests(): void {
  cachedPets = null;
  inFlight = null;
}

/**
 * The bundled-pet roster (`public/pets/index.json`), fetched once at runtime
 * and cached for the app session. Empty on the first render before the fetch
 * resolves (and stays empty if it fails) — callers that need to react to a
 * specific pet's bundled-ness should prefer `useBundledPetSlugs`.
 */
export function useBundledPets(): BundledPet[] {
  const [pets, setPets] = useState<BundledPet[]>(() => cachedPets ?? []);

  useEffect(() => {
    let cancelled = false;
    loadBundledPets().then((loaded) => {
      if (!cancelled) setPets(loaded);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return pets;
}

/** Same roster as `useBundledPets`, indexed by slug for O(1) "is this pet bundled?" checks. */
export function useBundledPetSlugs(): Set<string> {
  const pets = useBundledPets();
  return useMemo(() => new Set(pets.map((pet) => pet.slug)), [pets]);
}
