### PERP Engine CEX

## General Idea
This is the PERP engine that i am building, Centralized Exchange on solana. Here the user signs up, they are assigned a
pubkey, from previously generated MPC keys inside of the different kubernetes clusters
locally representing different cloud platforms in real life. This is done for security,
that at the time of signing any transaction, it happens directly between the clusters and
the frontend, therefore completely eliminating the backend as a point of attack. now this
is a centralized PERP engine, with a deposit architecture of every user has an account, and
it's data is maintained in the database, from which the funds are swept into a fat wallet.

## Withdrawals
- Then in case of withdrawals, it would be done thorugh the fat wallet, so this has to be highly guarded. 
- Some things that need to be kept in mind: 
a. user requests a withdraw, but the blockchian is down.
b. user spawns requests, and it succeeds to lock the balance after checking, may lead to inconsistencies if checking, updating, locking is happening in seperate db transaction, they should be atomic, for security. 

- For this, the two-phased commit is preferred: Ack based queues, 2 DB tables, and so on. 
- Now in this also, if the worker sends a transaction to blockchain, and then fails to pop that req from the pending queue, so there should be a transaciton signature attached to the req in db, that can verfy the status.
- there should be a limit on the number of requests a user can make for withdrawal in 24 hrs. 
