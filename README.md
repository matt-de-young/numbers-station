# Numbers Station

This service is a gRPC random number generator inpired by radio [Numbers Stations](https://en.wikipedia.org/wiki/Numbers_station). It was created only as a coding exercise & while this might be of use for multiple clients to have a time stable source of randomness, I can't think of any use cases for that. 

Once created, a station will stream a new random number every second. Stations can be created with a random or selected seed value, two stations with the same seed will always stream the same values. Once a station is created, the seed is secret. 

## TODOS:
- [ ] Telemetry
- [ ] Logging
- [x] Add a "current listeners" count to the station info
- [x] Automatically close stations after 1 hour of no listeners
- [x] Limit number of active stations
- [ ] Use Buf to manage protobuf
- [ ] Unit Tests
- [ ] Containerize
- [ ] Deploy
- [ ] CD
- [ ] Actual README
- [x] Server Reflection
- [x] Health Check
- [x] Refactor into multiple files
