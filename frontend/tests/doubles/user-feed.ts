/** The feed of a person (`user`): what happens to them wherever they look, told as events (their standing in a chat, a draft saved on another device, an approval that was decided, a route that changed), and the rows of their Inbox with its counters.
 *
 * One for the whole world, and the only one that takes a subscription to `user`: a part that has something to tell somebody calls `tell`,
 * and every connection of that person that follows their feed hears it. The Inbox is kept by the notifications double, which tells this
 * feed how far the person's Inbox has got (`inbox`) and sends its rows to the connections that follow it (`listeners`).
 */
import type { Part, Person, Subscriber } from './world.ts';

/** One connection that follows a person's feed, and the number of the last row of the Inbox it was sent. */
export interface Listening {
  subscriber: Subscriber;
  id: number;
  username: string;
  last: number;
}

/** What the feed asks of the Inbox when a connection joins: where it has got to, and the frame of what the connection missed after a number. */
export interface InboxView {
  seq: (username: string) => number;
  missed: (username: string, since: number) => Record<string, unknown>;
}

export class UserFeed implements Part {
  private listening: Listening[] = [];
  /** The Inbox the feed carries the rows of: put there by the world once it has both. */
  inbox: InboxView | null = null;

  /** An event for the person's own feed. */
  tell(username: string, data: Record<string, unknown>): void {
    for (const feed of this.listening) {
      if (feed.username === username) feed.subscriber.send({ op: 'event', id: feed.id, feed: 'user', data });
    }
  }

  /** An event for every connection that follows a feed, or for those of some people. */
  tellAll(data: Record<string, unknown>, users?: readonly string[]): void {
    for (const feed of this.listening) {
      if (users === undefined || users.includes(feed.username)) feed.subscriber.send({ op: 'event', id: feed.id, feed: 'user', data });
    }
  }

  /** The connections that follow the person's feed. */
  listeners(username: string): readonly Listening[] {
    return this.listening.filter((feed) => feed.username === username);
  }

  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const id = Number(frame.id);
    if (frame.op === 'sub' && frame.feed === 'user') {
      if (person === undefined) {
        subscriber.send({ op: 'error', id, code: 'forbidden', detail: 'user' });
        return true;
      }
      const username = person.me.username;
      const seq = this.inbox?.seq(username) ?? 0;
      const since = typeof frame.since === 'number' ? frame.since : null;
      // A number from after what the Inbox holds means it was put back: the reader loads it anew.
      const reload = since !== null && since > seq;
      const joined: Listening = { subscriber, id, username, last: reload ? seq : (since ?? seq) };
      this.listening.push(joined);
      subscriber.send({ op: 'subscribed', id, feed: 'user', seq, mode: reload ? 'reload' : 'delta' });
      if (!reload && since !== null && since < seq && this.inbox !== null) {
        subscriber.send({ op: 'sync', id, feed: 'user', prev: since, seq, data: this.inbox.missed(username, since) });
        joined.last = seq;
      }
      return true;
    }
    if (frame.op === 'unsub') {
      const before = this.listening.length;
      this.listening = this.listening.filter((feed) => !(feed.subscriber === subscriber && feed.id === id));
      if (this.listening.length === before) return false;
      subscriber.send({ op: 'closed', id, feed: 'user', reason: 'unsubscribed' });
      return true;
    }
    return false;
  }

  gone(subscriber: Subscriber): void {
    this.listening = this.listening.filter((feed) => feed.subscriber !== subscriber);
  }
}
