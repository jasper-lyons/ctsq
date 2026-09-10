require 'json'
require_relative 'helpers'
require_relative 'config'
require 'util/geo'

def main
  Geo.new.ping
  do_thing
  puts JSON.dump(DEBUG)
end
