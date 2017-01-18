This is a similar project to the LMAX disruptor.
The idea is the same - One sets up a series of 'processors' that are mapped to actual threads which receive events from a series of high-performance queues. Hence the name pipeline, since it is conceptually similar to a cpu pipeline.
This implementation extends those ideas further.

First off, why is this model better than having each core on a machine act as an independent server, or round-robining requests out to a set of identical handlers? The two big reasons are:

  * Cache coherency: Each logical unit of the handler will be on a given thread (possibly pinned to a given core). This means that the data and code used on that core will be limited to a subset of that used when handling the entire request.
  * Single threaded code: State that would otherwise be shared between a set of request handlers can instead be local to a single processing step in te request pipeline. What might otherwise be a shared hash table or redis instance on the machine is now just a simple hash table used by a single piece of code. This is great for simplicity and performance.

Now, aside from the regular reasons to use Rust over Java, here are some of the new features that will exist:

  * Multiple consumers: An event processor can be cloned and have multiple consumers from the same queue. While each queue holds the same multi-producer broadcasting to a group of single consumers, this lets a subset of the group act as a single 'unit' each individually claiming access to an event. (See my poor ascii art below).
  * Grouping multiple 'processors' in a single thread: During times of low volume, it doesn't really make sense for a bunch of threads to be waiting doing almost nothing.
    Event processors will get bunched together onto a single thread until load requires that event handlers resplit.
  * Automatic sharding: If possible for a given event processor, this library will allow for event handlers to split into distinct handlers on different thread when the load is too high. Similarly, when demand sufficiently decreases, split event-processors will be recombined.
  * Futures: This library will be able to represent results using futures, so it can easily operate with the rest of the ecosystem.

Here's some poor ascii art of what I mean by multi consumer:

  * LMAX disruptor: Each consumer (@) recieves each event once
```
         @
        /
-> @ -> -> @
        \
         @

```

  * Pipeline: Each Logical consumer receieves the event once, but a logical consumer might actually demultiplex the input over a set of consumers       
```
         @
        /
-> @ -> -> @ (really @+@+@)
        \
         @

```
Now, anyone looking at this code can see that most of the above isn't implemented yet - but the above is the plan for what this framework will allow.