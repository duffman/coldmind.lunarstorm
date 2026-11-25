// `lunarstorm:hostapi`: which of the host's API areas this program may use.
// Each active area is its own module, `lunarstorm:hostapi/<area>`.
import { ops } from 'lunarstorm:internal';

/**
 * The names of the active areas, in manifest order.
 * @type {readonly string[]}
 */
export const areas = Object.freeze([...ops.hostapi_areas]);
