/** A side thread beside the conversation: its own short exchange with the same agent, named after its thread, kept by a fork or closed.
 * The conversation's shared lane owns the aside's stream, composer and panel.
 */
export { SideLaneSlot } from '@/features/timeline/SideThreadLane';
